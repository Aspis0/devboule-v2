use crate::browser::delta::{between_input, Delta, Lists, Place};
use crate::browser::delta_input::{Briefly, OTHER_LINES};
use crate::browser::test_pages::{node, tree};
use crate::browser::test_support::{ax_property, ax_text, ax_with, ax_with_property};
use crate::browser::view::{Mode, View};
use crate::browser::view_walk::compact;
use serde_json::{json, Value};

const FIELD: u64 = 2;

fn place() -> Place {
    Place {
        url: "https://example.test/".to_owned(),
        title: Some("Search".to_owned()),
        document: 1,
    }
}

/// A page with a search form (the field and `in_form` links), a site navigation
/// (`elsewhere` links) and, when `options` is not zero, a listbox of suggestions.
fn page(value: &str, in_form: u64, elsewhere: u64, options: u64) -> View {
    let form_links: Vec<u64> = (0..in_form).map(|at| 20 + at).collect();
    let nav_links: Vec<u64> = (0..elsewhere).map(|at| 100 + at).collect();
    let suggestions: Vec<u64> = (0..options).map(|at| 200 + at).collect();
    let mut form_children = vec![FIELD];
    form_children.extend(&form_links);
    let mut nodes: Vec<Value> = vec![
        node(900, "RootWebArea", "Search", &[10, 11, 12]),
        node(10, "form", "Search", &form_children),
        ax_with_property(
            ax_with(
                node(FIELD, "textbox", "Search", &[]),
                "value",
                ax_text(value),
            ),
            ax_property("focused", "booleanOrUndefined", json!(true)),
        ),
        node(11, "navigation", "Site", &nav_links),
        node(12, "listbox", "Suggestions", &suggestions),
    ];
    nodes.extend(
        form_links
            .iter()
            .map(|id| node(*id, "link", &format!("In form {id}"), &[])),
    );
    nodes.extend(
        nav_links
            .iter()
            .map(|id| node(*id, "link", &format!("Elsewhere {id}"), &[])),
    );
    nodes.extend(
        suggestions
            .iter()
            .map(|id| node(*id, "option", &format!("Option {id}"), &[])),
    );
    compact(&tree(nodes), Mode::Interactive)
}

fn typed(before: &View, after: &View) -> Delta {
    between_input(before, after, &place(), &place(), Some(FIELD))
}

fn brief(delta: &Delta) -> &Briefly {
    match delta.changes.as_ref().expect("a change in one document") {
        Lists::Input(briefly) => briefly,
        Lists::Everything(_) => panic!("an input action is answered briefly"),
    }
}

#[test]
fn a_field_that_took_its_value_is_the_answer_and_nothing_else_is() {
    let delta = typed(&page("", 0, 0, 0), &page("WebView2", 0, 0, 0));

    assert_eq!(
        delta.target.as_deref(),
        Some(r#"  - textbox "Search" [value="WebView2"] [ref=e2]"#)
    );
    let json = serde_json::to_value(&delta).expect("serializes");
    for key in ["opened", "added", "changed", "removed", "more"] {
        assert!(json.get(key).is_none(), "{key} says nothing here: {json}");
    }
}

#[test]
fn the_field_is_not_listed_again_among_the_changes() {
    let delta = typed(&page("", 0, 0, 0), &page("WebView2", 0, 0, 0));

    assert!(brief(&delta).changed.is_empty(), "{:?}", brief(&delta));
    assert_eq!(
        delta.focused, None,
        "nor is it the focused node a second time"
    );
}

#[test]
fn the_popups_that_opened_come_next_and_are_cut_at_eight() {
    let delta = typed(&page("", 0, 0, 0), &page("Web", 0, 0, 12));

    let brief = brief(&delta);
    assert_eq!(brief.opened.len(), 8);
    assert_eq!(
        brief.opened[0], r#"  - option "Option 200" [ref=e200]"#,
        "in document order"
    );
    assert_eq!(brief.more, 4, "the four that did not fit are counted");
    assert!(
        brief.added.is_empty(),
        "an option is a popup line, not an addition"
    );
}

#[test]
fn the_rest_is_ten_lines_with_the_ones_near_the_field_first() {
    // A page of thirty links inside the field's own form: all of them are
    // news, and only the first ten are sent.
    let delta = typed(&page("", 0, 0, 0), &page("Web", 30, 0, 0));

    let brief = brief(&delta);
    assert_eq!(brief.added.len(), OTHER_LINES);
    assert_eq!(brief.more, 30 - OTHER_LINES, "and the rest are counted");
    assert!(
        brief
            .added
            .iter()
            .take(3)
            .all(|line| line.contains("In form")),
        "{:?}",
        brief.added
    );
}

#[test]
fn what_arrived_in_another_part_of_the_page_is_not_news() {
    // Typing into the field brought in three links of its own form and twenty-
    // five of the site navigation, which this page draws as the page is read.
    let delta = typed(&page("", 0, 0, 0), &page("Web", 3, 25, 0));

    let brief = brief(&delta);
    assert_eq!(
        brief.added,
        vec![
            r#"  - link "In form 20" [ref=e20]"#.to_owned(),
            r#"  - link "In form 21" [ref=e21]"#.to_owned(),
            r#"  - link "In form 22" [ref=e22]"#.to_owned(),
        ],
        "the navigation's links are chrome the page drew late, and nothing else"
    );
    assert_eq!(brief.more, 0, "and they are not counted as left out either");
}

#[test]
fn a_popup_from_another_part_of_the_page_is_still_news() {
    // The listbox of suggestions is the one thing a typed letter opens
    // somewhere else, and it is nowhere near the field's landmark.
    let delta = typed(&page("", 0, 0, 0), &page("Web", 0, 0, 3));

    let brief = brief(&delta);
    assert_eq!(brief.opened.len(), 3, "{:?}", brief.opened);
    assert!(brief.added.is_empty(), "a suggestion is not an addition");
}

#[test]
fn a_changed_line_comes_before_an_added_one() {
    let before = page("", 2, 0, 0);
    let mut after = page("Web", 12, 0, 0);
    // The one link both pages have is the only node that differs.
    after.nodes.iter_mut().for_each(|node| {
        if node.backend_id == 21 {
            node.line = r#"  - link "In form 21" [selected] [ref=e21]"#.to_owned();
        }
    });

    let delta = typed(&before, &after);

    // Ten lines in all: the change takes one, and ten additions share nine.
    assert_eq!(brief(&delta).changed.len(), 1, "{:?}", brief(&delta));
    assert_eq!(brief(&delta).added.len(), 9);
    assert_eq!(brief(&delta).more, 1);
}

#[test]
fn the_field_comes_before_everything_else_in_what_is_sent() {
    let delta = typed(&page("", 0, 0, 0), &page("Web", 3, 0, 3));

    let sent = serde_json::to_string(&delta).expect("serializes");
    let target = sent.find("\"target\"").expect("target");
    assert!(target < sent.find("\"opened\"").expect("opened"), "{sent}");
    assert!(target < sent.find("\"added\"").expect("added"), "{sent}");
}

#[test]
fn an_input_that_navigated_is_answered_like_any_navigation() {
    let from = place();
    let to = Place {
        url: "https://example.test/results".to_owned(),
        title: Some("Results".to_owned()),
        document: 2,
    };

    let delta = between_input(
        &page("", 0, 0, 0),
        &page("Web", 0, 30, 0),
        &from,
        &to,
        Some(FIELD),
    );

    assert!(delta.navigated);
    assert_eq!(delta.changes, None);
    assert!(delta.summary.is_some());
}
