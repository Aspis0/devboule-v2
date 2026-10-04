use super::*;
use crate::browser::ax::AxTree;
use crate::browser::test_pages::{front_page, report};
use crate::browser::test_support::{buttons, checkbox_tree, flat_tree};
use crate::browser::view::Mode;
use crate::browser::view_walk::compact;

fn place(url: &str) -> Place {
    Place {
        url: url.to_owned(),
        title: Some("Sign in".to_owned()),
        document: 1,
    }
}

fn view_of(tree: &AxTree) -> View {
    compact(tree, Mode::Interactive)
}

/// The lists of a delta inside one document, which is the only kind that has any.
fn lists(delta: &Delta) -> &Changes {
    match delta
        .changes
        .as_ref()
        .expect("a change inside one document has lists")
    {
        Lists::Everything(changes) => changes,
        Lists::Input(_) => panic!("this action lists everything"),
    }
}

#[test]
fn a_page_that_did_not_move_answers_with_no_lists_at_all() {
    let view = view_of(&flat_tree(&[("button", "Save")]));
    let delta = between(&view, &view, &place("a"), &place("a"), None);

    assert!(!delta.navigated);
    let lists = lists(&delta);
    assert!(lists.added.is_empty() && lists.removed.is_empty() && lists.changed.is_empty());
    assert_eq!(lists.added_more, 0);
    assert_eq!(delta.focused.as_deref(), None);
    assert_eq!(delta.dialog, None);
}

#[test]
fn a_node_that_appeared_and_one_that_did_not_are_named_on_their_own_lists() {
    let before = view_of(&flat_tree(&[("button", "Save")]));
    let after = view_of(&flat_tree(&[("button", "Save"), ("button", "Publish")]));

    let delta = between(&before, &after, &place("a"), &place("a"), None);
    assert_eq!(
        lists(&delta).added,
        vec!["- button \"Publish\" [ref=e2]".to_owned()]
    );
    assert!(lists(&delta).removed.is_empty());

    let back = between(&after, &before, &place("a"), &place("a"), None);
    assert_eq!(
        lists(&back).removed,
        vec!["- button \"Publish\" [ref=e2]".to_owned()]
    );
    assert!(lists(&back).added.is_empty());
}

#[test]
fn a_node_that_is_still_there_and_is_different_is_a_change_and_not_two_adds() {
    let before = view_of(&checkbox_tree("false"));
    let after = view_of(&checkbox_tree("true"));

    let delta = between(&before, &after, &place("a"), &place("a"), None);
    assert_eq!(
        lists(&delta).changed,
        vec!["- checkbox \"Remember\" [checked] [ref=e1]".to_owned()],
        "the same node, read twice: one change, not one add and one remove"
    );
    assert!(lists(&delta).added.is_empty() && lists(&delta).removed.is_empty());
}

#[test]
fn a_navigation_is_reported_as_one_and_the_lists_stay_empty() {
    let view = view_of(&flat_tree(&[("button", "Save")]));
    let delta = between(&view, &view, &place("a"), &place("b"), None);

    assert!(delta.navigated);
    assert_eq!(delta.url, "b");
}

#[test]
fn a_navigation_answers_with_what_the_new_page_is_and_not_with_all_of_it() {
    let before = view_of(&flat_tree(&[("button", "Save")]));
    let after = view_of(&report());

    let delta = between(&before, &after, &place("a"), &place("b"), None);

    assert!(delta.navigated);
    assert_eq!(delta.changes, None, "there is no before to compare with");
    assert_eq!(
        delta.summary,
        Some(Summary {
            headings: vec![
                "h1 \"Quarterly report\"".to_owned(),
                "h2 \"Totals\"".to_owned()
            ],
            links: 3,
            buttons: 2,
            fields: 1,
        })
    );
}

#[test]
fn a_big_page_is_summarized_and_not_listed() {
    let before = view_of(&buttons(1, "Save"));
    let after = view_of(&buttons(250, "Save"));

    let moved = between(&before, &after, &place("a"), &place("b"), None);
    let json = serde_json::to_value(&moved).expect("the delta serializes");

    assert!(json.get("added").is_none(), "{json}");
    assert!(json.get("addedMore").is_none(), "{json}");
    assert_eq!(json["summary"]["buttons"], 250);
    assert_eq!(json["navigated"], true);
}

#[test]
fn the_summary_names_at_most_five_headings_and_cuts_a_long_one() {
    let long = "x".repeat(200);
    let mut nodes: Vec<(&str, &str)> = vec![("heading", long.as_str())];
    nodes.extend((0..9).map(|_| ("heading", "More")));
    let after = view_of(&flat_tree(&nodes));

    let delta = between(
        &view_of(&AxTree::default()),
        &after,
        &place("a"),
        &place("b"),
        None,
    );

    let headings = delta.summary.expect("a navigation has a summary").headings;
    assert_eq!(headings.len(), 5);
    assert!(headings[0].chars().count() < 100, "{}", headings[0]);
}

#[test]
fn a_change_inside_one_page_has_no_summary_and_keeps_its_lists() {
    let before = view_of(&flat_tree(&[("button", "Save")]));
    let after = view_of(&flat_tree(&[("button", "Save"), ("button", "Publish")]));

    let delta = between(&before, &after, &place("a"), &place("a"), None);
    let json = serde_json::to_value(&delta).expect("the delta serializes");

    assert_eq!(delta.summary, None);
    assert_eq!(json["added"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        json["removed"],
        serde_json::json!([]),
        "the lists of a change are all there, empty or not"
    );
    assert!(json.get("summary").is_none(), "{json}");
}

#[test]
fn the_target_and_the_focus_are_still_read_off_the_page_that_was_landed_on() {
    let before = view_of(&AxTree::default());
    let after = view_of(&front_page());

    let delta = between(&before, &after, &place("a"), &place("b"), Some(904));

    assert_eq!(
        delta.target.as_deref(),
        Some("  - link \"login\" [ref=e904]")
    );
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
    assert_eq!(lists(&delta).removed.len(), 1);
}

#[test]
fn the_lists_are_capped_and_the_rest_is_counted_rather_than_dropped() {
    let before = view_of(&AxTree::default());
    let after = view_of(&buttons(60, "Save"));

    let delta = between(&before, &after, &place("a"), &place("a"), None);
    assert_eq!(lists(&delta).added.len(), LIST_CAP);
    assert_eq!(
        lists(&delta).added_more,
        20,
        "the other 20 are counted, not lost"
    );
}

fn moved_to(url: &str, document: u64) -> Place {
    Place {
        url: url.to_owned(),
        title: Some("Sign in".to_owned()),
        document,
    }
}

#[test]
fn a_page_that_moved_its_own_address_is_a_navigation_within_the_document() {
    let before = view_of(&flat_tree(&[("button", "Save")]));
    let after = view_of(&flat_tree(&[("button", "Save"), ("link", "Learn")]));

    let delta = between(
        &before,
        &after,
        &moved_to("https://react.dev/", 1),
        &moved_to("https://react.dev/learn", 1),
        None,
    );

    assert!(delta.navigated);
    assert!(delta.same_document);
    assert_eq!(delta.url, "https://react.dev/learn");
    assert_eq!(
        delta.changes, None,
        "the summary, not a list of what was added"
    );
    assert!(delta.summary.is_some());
    let json = serde_json::to_value(&delta).expect("serializes");
    assert_eq!(json["sameDocument"], true);
    assert_eq!(json["navigated"], true);
}

#[test]
fn a_new_document_is_a_navigation_that_is_not_within_one() {
    let view = view_of(&flat_tree(&[("button", "Save")]));

    let delta = between(
        &view,
        &view,
        &moved_to("https://example.test/a", 1),
        &moved_to("https://example.test/b", 2),
        None,
    );

    assert!(delta.navigated && !delta.same_document);
    let json = serde_json::to_value(&delta).expect("serializes");
    assert!(json.get("sameDocument").is_none(), "{json}");
}

#[test]
fn a_reload_is_a_navigation_at_the_same_address() {
    let view = view_of(&flat_tree(&[("button", "Save")]));

    let delta = between(
        &view,
        &view,
        &moved_to("https://example.test/a", 1),
        &moved_to("https://example.test/a", 2),
        None,
    );

    assert!(delta.navigated && !delta.same_document);
}

#[test]
fn a_title_that_changed_on_its_own_is_a_change_on_the_page_and_not_a_navigation() {
    let before = view_of(&flat_tree(&[("button", "Save")]));
    let after = view_of(&flat_tree(&[("button", "Save"), ("button", "Publish")]));
    let retitled = Place {
        title: Some("Draft saved".to_owned()),
        ..moved_to("https://example.test/a", 1)
    };

    let delta = between(
        &before,
        &after,
        &moved_to("https://example.test/a", 1),
        &retitled,
        None,
    );

    assert!(!delta.navigated);
    assert_eq!(delta.title.as_deref(), Some("Draft saved"));
    assert_eq!(lists(&delta).added.len(), 1);
}
