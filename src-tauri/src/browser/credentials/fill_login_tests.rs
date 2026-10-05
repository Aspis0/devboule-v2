//! The two host commands a saved login answers, driven through a page that
//! answers from a table: what may be used where, and what typing one actually
//! types.

use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

use super::fixtures::{
    asking, sign_in_page, tab, tab_named, typed, Over, FOREIGN, OTHER, PASSWORD, SECRET, SITE,
    USER, USERNAME,
};
use super::*;
use crate::browser::cdp::CdpError;
use crate::browser::credentials::secrets::fake::Act;

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
    let moved = sign_in_page().answering_with("Page.getFrameTree", |_| {
        json!({ "frameTree": { "frame": { "id": "main", "url": format!("{OTHER}/sign-in") } } })
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
    let page = sign_in_page().answering_with("Page.getFrameTree", move |_| {
        let first = seen.fetch_add(1, Ordering::SeqCst) == 0;
        json!({"frameTree": {"frame": {
            "id": if first { "main" } else { "next-document" },
            "url": format!("{SITE}/sign-in"),
        }}})
    });

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

/// A field in a frame of another site is typed into a page that is still the
/// tab's own, and what the tab says of it a command later is still scrubbed:
/// the tab, not the field's frame, is what "leaves the site".
#[test]
fn a_password_typed_in_another_sites_frame_is_still_scrubbed_on_the_next_command() {
    // Its own value: the scrub removes every tab's values from every answer,
    // so a password other tests also type would hide a failure here.
    let password = "SENTINEL-FRAME-PW-9d2e";
    let tab = tab_named("fill-in-a-frame");
    let over = Over::new();
    let saved = over
        .vault
        .create("Ad account", &[OTHER.to_owned()], USER, password)
        .expect("the login was saved")
        .id;
    tauri::async_runtime::block_on(fill(
        &over.vault,
        &tab,
        &sign_in_page(),
        &asking(json!({"entryId": saved, "passwordRef": FOREIGN})),
    ))
    .expect("the login was typed into the child frame");

    // What `dispatch` does before the next command about this tab.
    scrub::left(&tab.browser_id, origin_of(&tab).as_deref());
    let mut afterwards = json!({ "text": format!("the page showed {password}") });
    scrub::clean(&mut afterwards);

    assert!(
        !afterwards.to_string().contains(password),
        "the frame's site was taken for the page's: {afterwards}"
    );
    scrub::forget(&tab.browser_id);
}

/// The store is asked on a thread of its own, not on the one running the
/// command: an OS store can wait on a person.
#[test]
fn the_password_is_read_off_the_thread_that_runs_the_command() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);
    let tab = tab_named("fill-off-thread");

    tauri::async_runtime::block_on(fill(
        &over.vault,
        &tab,
        &sign_in_page(),
        &asking(json!({"entryId": saved, "passwordRef": PASSWORD})),
    ))
    .expect("the login was typed");

    let readers = over.store.readers();
    assert_eq!(readers.len(), 1, "the store was read once");
    assert_ne!(readers[0], std::thread::current().id());
    scrub::forget(&tab.browser_id);
}

/// A value is registered for the scrub once the checks have passed, and not
/// kept when the page refused it.
#[test]
fn a_password_the_page_refused_is_not_left_registered() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);
    let tab = tab_named("fill-refused-send");
    let page = sign_in_page().refusing(
        "Input.insertText",
        CdpError::Refused("the page said no".to_owned()),
    );

    tauri::async_runtime::block_on(fill(
        &over.vault,
        &tab,
        &page,
        &asking(json!({"entryId": saved, "passwordRef": PASSWORD})),
    ))
    .expect_err("the insert was refused");

    assert_eq!(scrub::typed_of(&tab.browser_id), 0);
}

/// A send that ran out of time may have gone in, and the scrub keeps covering
/// it.
#[test]
fn a_password_whose_send_ran_out_of_time_stays_registered() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);
    let tab = tab_named("fill-timed-out-send");
    let page = sign_in_page().refusing("Input.insertText", CdpError::OutOfTime);

    tauri::async_runtime::block_on(fill(
        &over.vault,
        &tab,
        &page,
        &asking(json!({"entryId": saved, "passwordRef": PASSWORD})),
    ))
    .expect_err("the insert ran out of time");

    assert_eq!(scrub::typed_of(&tab.browser_id), 1);
    scrub::forget(&tab.browser_id);
}

/// A check that refuses before the insert leaves nothing registered, though the
/// store has been read by then.
#[test]
fn a_password_stopped_by_the_last_check_is_not_registered() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);
    let tab = tab_named("fill-stopped-check");
    let page = sign_in_page().refusing_after(
        "Runtime.callFunctionOn",
        "DOM.describeNode",
        CdpError::StaleRef,
    );

    tauri::async_runtime::block_on(fill(
        &over.vault,
        &tab,
        &page,
        &asking(json!({"entryId": saved, "passwordRef": PASSWORD})),
    ))
    .expect_err("the field went away");

    assert_eq!(over.store.readers().len(), 1, "the store had been read");
    assert_eq!(scrub::typed_of(&tab.browser_id), 0);
}

/// By the time the page is asked to take the value the scrub already holds it,
/// so nothing the page does in between can be answered uncovered.
#[test]
fn the_scrub_holds_the_value_by_the_time_it_is_sent() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);
    let tab = tab_named("fill-held-when-sent");
    let id = tab.browser_id.clone();
    let seen = std::sync::Arc::new(AtomicUsize::new(usize::MAX));
    let looked = std::sync::Arc::clone(&seen);
    let page = sign_in_page().during("Input.insertText", move || {
        looked.store(scrub::typed_of(&id), Ordering::SeqCst);
    });

    tauri::async_runtime::block_on(fill(
        &over.vault,
        &tab,
        &page,
        &asking(json!({"entryId": saved, "passwordRef": PASSWORD})),
    ))
    .expect("the login was typed");

    assert_eq!(seen.load(Ordering::SeqCst), 1);
    scrub::forget(&tab.browser_id);
}
