//! What a typed password must not travel in, once it is in a page.

use serde_json::json;

use super::{clean, clean_text, forget, left, remember, typed_of};

const SECRET: &str = "sentinel-typed-password";
/// The site a value was typed into, and a different one.
const SITE: &str = "https://shop.example.test";
const ELSEWHERE: &str = "https://ads.example.test";

/// Nothing this process has typed into a tab comes back out of an answer.
#[test]
fn a_typed_password_is_taken_out_of_every_string_of_an_answer() {
    let tab = "scrub-answer";
    remember(tab, SITE, SECRET);

    let mut answer = json!({
        "view": format!("- textbox \"Password\" [value=\"{SECRET}\"] [ref=e14]"),
        "delta": {"added": [format!("- paragraph \"you typed {SECRET}\"")], "navigated": false},
        "entries": [{"level": "info", "text": format!("the page said {SECRET}")}],
        "text": format!("your password is {SECRET}"),
        "url": "https://example.test/sign-in",
        "dropped": 0,
        "truncated": false,
    });
    clean(&mut answer);

    let said = answer.to_string();
    assert!(!said.contains(SECRET), "the answer still holds it: {said}");
    assert_eq!(
        answer["view"],
        "- textbox \"Password\" [value=\"[hidden]\"] [ref=e14]"
    );
    assert!(
        answer["delta"]["navigated"].eq(&json!(false)),
        "the shape is kept"
    );
    assert_eq!(answer["dropped"], json!(0), "a number is not text");
    forget(tab);
}

/// A refusal is text too, and a page can put the value into an error it causes.
#[test]
fn a_typed_password_is_taken_out_of_a_message() {
    let tab = "scrub-message";
    remember(tab, SITE, SECRET);

    let mut refusal = format!("the page refused {SECRET} twice");
    clean_text(&mut refusal);

    assert_eq!(refusal, "the page refused [hidden] twice");
    forget(tab);
}

/// Two tabs, two logins: an answer about one page never carries the other
/// page's password either, because the scrub is this process's, not a field's.
#[test]
fn every_typed_password_is_taken_out_whatever_tab_it_was_typed_into() {
    let one = "scrub-two-one";
    let two = "scrub-two-two";
    remember(one, SITE, "sentinel-first-password");
    remember(two, ELSEWHERE, "sentinel-second-password");

    let mut answer = json!({"text": "sentinel-first-password and sentinel-second-password"});
    clean(&mut answer);

    assert_eq!(answer["text"], json!("[hidden] and [hidden]"));
    forget(one);
    forget(two);
}

/// The value is held while the tab is on the site it was typed into, and
/// dropped once the tab is on another site — which is what leaving the login
/// behind means. A redirect that keeps the site keeps the scrub too: the page
/// that lands may still print what was typed into the one before.
#[test]
fn the_value_is_forgotten_only_when_the_tab_leaves_the_site() {
    let tab = "scrub-left";
    remember(tab, SITE, SECRET);
    assert_eq!(typed_of(tab), 1, "it is held while that site is there");

    left(tab, Some(SITE));
    assert_eq!(typed_of(tab), 1, "the same site keeps it");

    left(tab, Some(ELSEWHERE));
    assert_eq!(
        typed_of(tab),
        0,
        "another site is on screen, so the value goes"
    );

    left(tab, None);
    assert_eq!(typed_of(tab), 0, "an unreadable address holds nothing");
}

/// An address no site can be compared on is not the site the value was typed
/// into, so it drops the value like any other address.
#[test]
fn an_unreadable_address_after_a_fill_forgets_the_value() {
    // A value of this test's own: what a concurrent test holds is scrubbed from
    // every answer, so a shared one could not tell "dropped" from "not mine".
    const DROPPED: &str = "sentinel-address-dropped";
    let tab = "scrub-opaque";
    remember(tab, SITE, DROPPED);

    left(tab, None);

    assert_eq!(typed_of(tab), 0);
    let mut answer = json!({"text": DROPPED});
    clean(&mut answer);
    assert_eq!(
        answer["text"],
        json!(DROPPED),
        "and it is no longer scrubbed"
    );
    forget(tab);
}

/// A close is the other end of a tab's life, and it forgets too.
#[test]
fn closing_a_tab_forgets_what_was_typed_into_it() {
    let tab = "scrub-close";
    remember(tab, SITE, SECRET);
    remember("scrub-close-other", SITE, SECRET);

    forget(tab);

    assert_eq!(typed_of(tab), 0);
    assert_eq!(
        typed_of("scrub-close-other"),
        1,
        "another tab keeps its own"
    );
    forget("scrub-close-other");
}

/// Typing the same password into a tab twice holds one copy of it, not two:
/// the value is the same and the list has no reason to grow.
#[test]
fn one_value_typed_twice_is_held_once() {
    let tab = "scrub-twice";
    remember(tab, SITE, SECRET);
    remember(tab, SITE, SECRET);

    assert_eq!(typed_of(tab), 1);
    forget(tab);
}

/// Two sites, one after the other, in the same tab: the value typed on the
/// first is dropped when the tab reaches the second, and the second's own
/// value is what remains.
#[test]
fn a_tab_that_leaves_one_site_keeps_only_the_value_of_the_one_it_is_on() {
    // Two values of this test's own, for the same reason as above.
    const LEFT_BEHIND: &str = "sentinel-left-behind";
    const ARRIVED: &str = "sentinel-arrived";
    let tab = "scrub-move";
    remember(tab, SITE, LEFT_BEHIND);
    remember(tab, ELSEWHERE, ARRIVED);

    left(tab, Some(ELSEWHERE));

    assert_eq!(typed_of(tab), 1);
    let mut answer = json!({"text": format!("{LEFT_BEHIND} and {ARRIVED}")});
    clean(&mut answer);
    assert_eq!(
        answer["text"],
        json!(format!("{LEFT_BEHIND} and [hidden]")),
        "the value of the site left behind is no longer scrubbed"
    );
    forget(tab);
}

/// An answer when nothing has been typed is left alone: the walk runs on every
/// command, and a password nothing is in cannot be in it.
#[test]
fn an_answer_with_nothing_typed_is_unchanged() {
    assert_eq!(typed_of("scrub-never-used"), 0);
    let mut answer = json!({"text": "a page said nothing secret", "view": "- button \"Sign in\""});
    let before = answer.clone();
    clean(&mut answer);
    assert_eq!(answer, before);
}
