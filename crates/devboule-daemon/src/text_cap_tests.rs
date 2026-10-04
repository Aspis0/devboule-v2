//! Tests for one topic: the budget one tool row's text is held to.

use super::{capped, clip, shown, MAX_TEXT_BYTES, TRUNCATION_MARKER};

#[test]
fn an_answer_within_the_budget_is_shown_whole() {
    assert_eq!(capped(""), "");
    assert_eq!(capped("2 tabs"), "2 tabs");
    let exact = "a".repeat(MAX_TEXT_BYTES);
    assert_eq!(capped(&exact), exact);
}

#[test]
fn an_answer_over_the_budget_is_cut_and_marked() {
    let over = "a".repeat(MAX_TEXT_BYTES + 1);
    assert_eq!(
        capped(&over),
        format!("{}{TRUNCATION_MARKER}", "a".repeat(MAX_TEXT_BYTES))
    );
}

#[test]
fn the_cut_lands_on_a_character_boundary() {
    // Three-byte characters against a budget that is not a multiple of three:
    // a cut at the budget itself would land inside one.
    let over = "\u{20ac}".repeat(MAX_TEXT_BYTES);
    let kept = capped(&over)
        .strip_suffix(TRUNCATION_MARKER)
        .expect("a cut answer says so")
        .to_string();
    assert_eq!(kept.len(), MAX_TEXT_BYTES - 1);
    assert_eq!(kept.chars().count(), MAX_TEXT_BYTES / 3);
}

#[test]
fn a_body_the_caller_already_cut_is_marked_at_the_budget() {
    // Exactly at the budget: there is nothing left to cut, but the row still
    // shows less than the source held, and only the caller knows that.
    let exact = "a".repeat(MAX_TEXT_BYTES);
    assert_eq!(shown(&exact, false), exact);
    assert_eq!(shown(&exact, true), format!("{exact}{TRUNCATION_MARKER}"));
}

#[test]
fn clip_honours_any_budget_and_never_splits_a_character() {
    assert_eq!(clip("abc", 10), "abc");
    assert_eq!(clip("abc", 0), "");
    assert_eq!(clip("abc", 2), "ab");
    assert_eq!(clip("\u{20ac}\u{20ac}", 4), "\u{20ac}");
    assert_eq!(clip("\u{20ac}\u{20ac}", 1), "");
}
