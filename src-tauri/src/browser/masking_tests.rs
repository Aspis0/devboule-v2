//! What a password field must not leak, through the commands that hand a page
//! to an agent.
//!
//! The fixture is this runtime's own answer for a filled
//! `<input type=password>`: role `textbox`, no `protected` property, and a
//! value of one U+2022 per character (captured on this machine, in
//! `test_support::ax_sign_in_with_password`). Nothing in the tree therefore says
//! "secret", and the shape of the value is the leak — its length is the
//! password's length. The one fact that does say it is in the DOM, so every
//! answer here comes from a page that answers `DOM.describeNode` the way this
//! engine does.
//!
//! Every assertion about an answer goes through [`masked`], which is written
//! against what the fixture really holds: the runtime's own bullet run and the
//! password itself must be absent, and the fixed marker must be present. An
//! assertion about the password alone would pass with no masking at all,
//! because the runtime replaced it before this app ever saw it.

use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;
use serde_json::Value;

use devboule_protocol::{BrowserError, BrowserErrorCode};

use super::cdp::CdpError;
use super::commands::on_tab;
use super::deadline::Deadline;
use super::mask::SHOWN;
use super::test_support::{
    ax_fixture, ax_sign_in_with_password, ax_unnamed_password_field, masked_by_runtime, parked_tab,
    sign_in_markup, FakePage,
};

/// Typed into the field by the tests below. No answer may carry it, and none may
/// carry a bullet run of its length either.
const SECRET: &str = "SENTINEL-PW-7f3a";
/// The password field's `backendDOMNodeId` in the fixtures.
const PASSWORD: u64 = 14;

/// What this runtime reports for `SECRET`.
fn runtime_mask() -> String {
    masked_by_runtime(SECRET.chars().count())
}

/// What the page's own text would say about the secret, which is what a reader
/// that walks a `<textarea>`'s value reports as the page's prose.
const PAGE_PROSE: &str = "Sign in to your account";

fn run(page: &FakePage, command: &str, args: Value) -> Result<Value, BrowserError> {
    let tab = parked_tab("tab-1");
    let mut args = args;
    args["browserId"] = json!("tab-1");
    tauri::async_runtime::block_on(on_tab(
        &tab,
        page,
        command,
        &args,
        Deadline::in_(Duration::from_secs(10)),
    ))
}

/// A page whose password field is filled, and whose markup says so.
fn filled() -> FakePage {
    FakePage::new()
        .answering(
            "Accessibility.getFullAXTree",
            ax_sign_in_with_password(&runtime_mask()),
        )
        .answering_with("DOM.describeNode", sign_in_markup)
        .answering("DOM.resolveNode", json!({ "object": { "objectId": "7" } }))
        .answering(
            "Runtime.callFunctionOn",
            json!({ "result": { "value": true } }),
        )
}

fn view_of(answered: &Value) -> &str {
    answered["view"].as_str().expect("a view is text")
}

/// The one assertion every answer that can carry a value makes: what the
/// fixture really holds is absent, and the marker that says "there was
/// something here" is what appears instead.
fn masked(answer: &str, what: &str) {
    assert!(
        !answer.contains(&runtime_mask()),
        "{what} would print the password's length: {answer}"
    );
    assert!(
        !answer.contains(SECRET),
        "{what} printed the password: {answer}"
    );
    assert!(
        answer.contains(SHOWN),
        "{what} does not even say a field was filled: {answer}"
    );
}

#[test]
fn a_snapshot_marks_the_field_and_neither_its_value_nor_its_length() {
    let answered = run(&filled(), "snapshot", json!({})).expect("answered");

    let view = view_of(&answered);
    assert!(
        view.contains(&format!("[ref=e{PASSWORD}]")),
        "the password field is still in the view: {view}"
    );
    masked(view, "the view");
}

#[test]
fn an_ordinary_field_still_shows_what_it_says() {
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering_with("DOM.describeNode", sign_in_markup);

    let answered = run(&page, "snapshot", json!({})).expect("answered");

    let view = view_of(&answered);
    assert!(
        view.contains("value=\"person@example.test\""),
        "masking is about what the markup says, not about every value: {view}"
    );
    assert!(
        !view.contains(SHOWN),
        "and an ordinary value is never marked: {view}"
    );
}

#[test]
fn a_field_the_page_will_not_describe_is_marked() {
    // The DOM answers for every node a tree carries, and every command that
    // acts on a field asks the DOM for it. A page that refuses here is not
    // answering at all, which is not evidence that the field is ordinary.
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .refusing("DOM.describeNode", CdpError::Refused("no DOM".to_owned()));

    let answered = run(&page, "snapshot", json!({})).expect("answered");

    let view = view_of(&answered);
    assert!(
        view.contains(&format!("value=\"{SHOWN}\"")),
        "an undescribed value is not printed: {view}"
    );
    assert!(
        !view.contains("person@example.test"),
        "the value that was there is not read back: {view}"
    );
}

#[test]
fn a_deadline_spent_asking_the_page_about_fields_is_a_timeout() {
    // The tree read is what spends the budget here: the page takes its time
    // over that call and none over the ones after it, which is the only way to
    // reach the masking step with nothing left to spend. An answer that came
    // back masked would say a page looked fine because every lookup failed.
    let page = FakePage::new()
        .answering(
            "Accessibility.getFullAXTree",
            ax_sign_in_with_password(&runtime_mask()),
        )
        .answering_with("DOM.describeNode", sign_in_markup)
        .during("Accessibility.getFullAXTree", || {
            std::thread::sleep(Duration::from_millis(80))
        });
    let tab = parked_tab("tab-1");

    let refused = tauri::async_runtime::block_on(on_tab(
        &tab,
        &page,
        "snapshot",
        &json!({ "browserId": "tab-1" }),
        Deadline::in_(Duration::from_millis(20)),
    ))
    .expect_err("the command is out of time");

    assert_eq!(
        refused.code,
        BrowserErrorCode::Timeout,
        "a deadline that ran out is a timeout and not an answer: {}",
        refused.message
    );
    assert_eq!(
        page.called("DOM.describeNode"),
        0,
        "and the budget was gone before the first field was even asked about"
    );
}

#[test]
fn a_value_with_no_node_to_ask_about_is_marked() {
    // The view needs a backend id to address a node, so such a node never
    // reaches an answer today. It is marked anyway: a rule that holds only
    // because of what the view happens to skip is not a rule the next reader
    // of the tree can rely on.
    let nameless = json!({ "nodes": [
        { "nodeId": "1", "ignored": false,
          "role": { "value": "RootWebArea" }, "name": { "value": "Test page" },
          "childIds": ["2"] },
        { "nodeId": "2", "ignored": false,
          "role": { "value": "textbox" }, "name": { "value": "Passphrase" },
          "value": { "value": runtime_mask() } },
    ]});
    let page = FakePage::new().answering("Accessibility.getFullAXTree", nameless);

    let tree = tauri::async_runtime::block_on(super::commands::tree_of(&page)).expect("the tree");

    assert_eq!(
        tree.nodes[1]
            .value
            .as_ref()
            .map(|value| value.text())
            .unwrap_or_default(),
        SHOWN,
        "a value with no id to ask about is marked, not kept"
    );
}

#[test]
fn a_page_that_runs_out_of_time_is_answered_as_a_timeout() {
    // Masking asks the page about every valued node. A caller whose budget is
    // gone needs that said: an answer that looks fine because every lookup
    // failed is an answer that hides its own failure.
    let page = FakePage::new()
        .answering(
            "Accessibility.getFullAXTree",
            ax_sign_in_with_password(&runtime_mask()),
        )
        .refusing("DOM.describeNode", CdpError::OutOfTime);

    let refused = run(&page, "snapshot", json!({})).expect_err("the command is out of time");

    assert_eq!(
        refused.code,
        BrowserErrorCode::Timeout,
        "a deadline that ran out is a timeout and not an answer: {}",
        refused.message
    );
}

#[test]
fn a_fill_into_a_password_field_answers_without_the_value_or_its_length() {
    let tab = parked_tab("tab-1");
    // What a real fill sees: an empty field before the text goes in, and the
    // runtime's mask once it has.
    let reads = Mutex::new(0u32);
    let page = FakePage::new()
        .answering_with("Accessibility.getFullAXTree", move |_| {
            let mut reads = reads.lock().expect("the read count is poisoned");
            *reads += 1;
            let value = if *reads == 1 {
                String::new()
            } else {
                runtime_mask()
            };
            ax_sign_in_with_password(&value)
        })
        .answering_with("DOM.describeNode", sign_in_markup)
        .answering("DOM.resolveNode", json!({ "object": { "objectId": "7" } }))
        .answering(
            "Runtime.callFunctionOn",
            json!({ "result": { "value": true } }),
        );

    let answered = tauri::async_runtime::block_on(on_tab(
        &tab,
        &page,
        "fill",
        &json!({ "browserId": "tab-1", "ref": format!("e{PASSWORD}"), "text": SECRET }),
        Deadline::in_(Duration::from_secs(10)),
    ))
    .expect("answered");

    let target = answered["delta"]["target"]
        .as_str()
        .expect("the delta names the field it filled");
    masked(target, "the delta's target line");
}

#[test]
fn a_batch_step_into_a_password_field_answers_the_same_way() {
    let tab = parked_tab("tab-1");
    // The page moves under the batch exactly as it does under the step: empty
    // before the text goes in, and the runtime's mask once it has.
    let reads = Mutex::new(0u32);
    let page = FakePage::new()
        .answering_with("Accessibility.getFullAXTree", move |_| {
            let mut reads = reads.lock().expect("the read count is poisoned");
            *reads += 1;
            let value = if *reads == 1 {
                String::new()
            } else {
                runtime_mask()
            };
            ax_sign_in_with_password(&value)
        })
        .answering_with("DOM.describeNode", sign_in_markup)
        .answering("DOM.resolveNode", json!({ "object": { "objectId": "7" } }))
        .answering(
            "Runtime.callFunctionOn",
            json!({ "result": { "value": true } }),
        );

    let answered = tauri::async_runtime::block_on(on_tab(
        &tab,
        &page,
        "act",
        &json!({ "browserId": "tab-1", "steps": [
            { "command": "fill", "ref": format!("e{PASSWORD}"), "text": SECRET }
        ] }),
        Deadline::in_(Duration::from_secs(10)),
    ))
    .expect("answered");

    let changed = answered["delta"]["changed"]
        .as_array()
        .expect("the batch names the lines that changed");
    assert_eq!(changed.len(), 1, "the one field it filled");
    masked(
        changed[0].as_str().expect("a line is text"),
        "the batch's changed line",
    );
}

#[test]
fn a_find_names_the_field_it_found_by_its_markup_and_not_by_its_value() {
    // The field has no name of its own, so a `find` that found it says what
    // its markup says — which is `type=password`, the one fact a caller needs
    // and one that says nothing about the password.
    let page = FakePage::new()
        .answering(
            "Accessibility.getFullAXTree",
            ax_unnamed_password_field(&runtime_mask()),
        )
        .answering_with("DOM.describeNode", sign_in_markup);

    let answered = run(&page, "find", json!({ "query": "the field" })).expect("answered");

    let refs = answered["matches"]
        .as_array()
        .expect("matches is a list")
        .iter()
        .filter_map(|hit| hit["ref"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    assert_eq!(
        refs,
        vec![format!("e{PASSWORD}")],
        "the field is findable by what it is, and only that field answered"
    );
    assert_eq!(
        answered["matches"][0]["detail"]
            .as_str()
            .unwrap_or_default(),
        "type=password name=pass",
        "what the answer says about the field it found"
    );
}

#[test]
fn a_find_that_cannot_read_the_markup_describes_it_without_its_value() {
    // With no markup to describe it by, a nameless field is described by the
    // value the runtime reported — which is where a password would print
    // itself one bullet at a time.
    let page = FakePage::new()
        .answering(
            "Accessibility.getFullAXTree",
            ax_unnamed_password_field(&runtime_mask()),
        )
        .refusing("DOM.describeNode", CdpError::Refused("no DOM".to_owned()));

    let answered = run(&page, "find", json!({ "query": "the field" })).expect("answered");

    assert_eq!(
        answered["matches"][0]["detail"]
            .as_str()
            .unwrap_or_default(),
        format!("placeholder=\"{SHOWN}\""),
        "the fallback description is the marker, never the value"
    );
    masked(&answered.to_string(), "the find's answer");
}

/// `read_text` never reads the accessible tree, so the mask above cannot reach
/// it. Its own guard is a predicate in the page-side function, and this is the
/// only fact about that function a fake page can be honest about: a page-side
/// walk that skips the control the page marked as holding a password reports
/// the page's own words, and one that does not skip it reports the value too.
#[test]
fn the_page_side_reader_reports_no_password_because_the_page_asked_had_none() {
    let page = FakePage::new().answering_with("Runtime.evaluate", |asked| {
        let expression = asked["expression"].as_str().unwrap_or_default();
        // A guard that is defined and never called skips nothing, so both
        // halves have to be there: the marks it reads, and a call on each of
        // the two walks the function makes.
        const MARKS: &str = "type === \"password\"";
        const WALKS: [&str; 2] = ["secret(current)", "secret(node)"];
        let skips_passwords =
            expression.contains(MARKS) && WALKS.iter().all(|walk| expression.contains(walk));
        let mut text = String::from(PAGE_PROSE);
        if !skips_passwords {
            text.push('\n');
            text.push_str(SECRET);
        }
        json!({ "result": { "type": "string", "value": text } })
    });

    let answered = run(&page, "read_text", json!({})).expect("answered");

    assert_eq!(
        answered["text"], PAGE_PROSE,
        "the page's own words, and nothing else"
    );
    assert!(
        !answered.to_string().contains(SECRET),
        "a page-side walk that reads a password control hands the password back \
         as the page's prose"
    );
}
