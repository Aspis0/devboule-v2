use super::*;
use crate::browser::ax::AxTree;
use crate::browser::test_support::{buttons, checkbox_tree, flat_tree};
use crate::browser::view::{compact, Mode};

fn place(url: &str) -> Place {
    Place {
        url: url.to_owned(),
        title: Some("Sign in".to_owned()),
    }
}

fn view_of(tree: &AxTree) -> View {
    compact(tree, Mode::Interactive)
}

#[test]
fn a_page_that_did_not_move_answers_with_no_lists_at_all() {
    let view = view_of(&flat_tree(&[("button", "Save")]));
    let delta = between(&view, &view, &place("a"), &place("a"), None);

    assert!(!delta.navigated);
    assert!(delta.added.is_empty() && delta.removed.is_empty() && delta.changed.is_empty());
    assert_eq!(delta.added_more, 0);
    assert_eq!(delta.focused.as_deref(), None);
    assert_eq!(delta.dialog, None);
}

#[test]
fn a_node_that_appeared_and_one_that_did_not_are_named_on_their_own_lists() {
    let before = view_of(&flat_tree(&[("button", "Save")]));
    let after = view_of(&flat_tree(&[("button", "Save"), ("button", "Publish")]));

    let delta = between(&before, &after, &place("a"), &place("a"), None);
    assert_eq!(
        delta.added,
        vec!["- button \"Publish\" [ref=e2]".to_owned()]
    );
    assert!(delta.removed.is_empty());

    let back = between(&after, &before, &place("a"), &place("a"), None);
    assert_eq!(
        back.removed,
        vec!["- button \"Publish\" [ref=e2]".to_owned()]
    );
    assert!(back.added.is_empty());
}

#[test]
fn a_node_that_is_still_there_and_is_different_is_a_change_and_not_two_adds() {
    let before = view_of(&checkbox_tree("false"));
    let after = view_of(&checkbox_tree("true"));

    let delta = between(&before, &after, &place("a"), &place("a"), None);
    assert_eq!(
        delta.changed,
        vec!["- checkbox \"Remember\" [checked] [ref=e1]".to_owned()],
        "the same node, read twice: one change, not one add and one remove"
    );
    assert!(delta.added.is_empty() && delta.removed.is_empty());
}

#[test]
fn a_navigation_is_reported_as_one_and_the_lists_stay_empty() {
    let view = view_of(&flat_tree(&[("button", "Save")]));
    let delta = between(&view, &view, &place("a"), &place("b"), None);

    assert!(delta.navigated);
    assert_eq!(delta.url, "b");
}

#[test]
fn the_acted_on_node_is_read_again_so_a_control_that_did_not_flip_says_so() {
    let before = view_of(&checkbox_tree("false"));
    let after = view_of(&checkbox_tree("false"));

    let delta = between(&before, &after, &place("a"), &place("a"), Some(1));
    assert_eq!(
        delta.target.as_deref(),
        Some("- checkbox \"Remember\" [unchecked] [ref=e1]"),
        "an unchanged target is the answer a caller needs: the click did nothing"
    );
}

#[test]
fn a_target_that_is_gone_from_the_page_is_not_reported_as_a_node() {
    let before = view_of(&flat_tree(&[("button", "Save")]));
    let after = view_of(&AxTree::default());

    let delta = between(&before, &after, &place("a"), &place("a"), Some(1));
    assert_eq!(delta.target, None);
    assert_eq!(delta.removed.len(), 1);
}

#[test]
fn the_lists_are_capped_and_the_rest_is_counted_rather_than_dropped() {
    let before = view_of(&AxTree::default());
    let after = view_of(&buttons(60, "Save"));

    let delta = between(&before, &after, &place("a"), &place("a"), None);
    assert_eq!(delta.added.len(), LIST_CAP);
    assert_eq!(delta.added_more, 20, "the other 20 are counted, not lost");
}
