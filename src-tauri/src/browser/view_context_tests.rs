use crate::browser::test_pages::{encyclopedia, front_page, node, tree};
use crate::browser::view::{Mode, View, ViewNode};
use crate::browser::view_walk::compact;

fn at(view: &View, backend_id: u64) -> &ViewNode {
    view.nodes
        .iter()
        .find(|node| node.backend_id == backend_id)
        .expect("the node is a line of the view")
}

#[test]
fn a_landmark_the_page_did_not_name_is_still_the_part_of_the_page_a_node_is_in() {
    let view = compact(&encyclopedia(), Mode::Interactive);

    let field = at(&view, 905);
    assert_eq!(field.context, "in search");
    assert_eq!(field.landmark, "search");
    // The banner above the search landmark is not the nearest one.
    assert_eq!(at(&view, 903).landmark, "banner");
    assert_eq!(at(&view, 909).context, r#"in navigation "Contents""#);
}

#[test]
fn a_row_the_page_did_not_name_is_named_by_what_it_says() {
    let view = compact(&front_page(), Mode::Interactive);

    let comments = at(&view, 917);
    assert_eq!(comments.context, r#"in row "1. First story 12 comments""#);
    assert_eq!(comments.landmark, "", "the table is not a landmark");
}

#[test]
fn the_entry_comes_before_the_landmark_it_is_in() {
    let page = tree(vec![
        node(900, "RootWebArea", "", &[1]),
        node(1, "navigation", "menu", &[2]),
        node(2, "listitem", "", &[3]),
        node(3, "link", "Docs", &[4]),
        node(4, "StaticText", "Docs", &[]),
    ]);
    let view = compact(&page, Mode::Interactive);

    assert_eq!(
        at(&view, 3).context,
        r#"in listitem "Docs", in navigation "menu""#
    );
    assert_eq!(at(&view, 3).landmark, "navigation");
}

#[test]
fn a_form_or_a_region_is_a_landmark_only_when_it_is_named() {
    let page = tree(vec![
        node(900, "RootWebArea", "", &[1, 2]),
        node(1, "form", "", &[3]),
        node(2, "region", "Results", &[4]),
        node(3, "button", "Send", &[]),
        node(4, "button", "Next", &[]),
    ]);
    let view = compact(&page, Mode::Interactive);

    assert_eq!(at(&view, 3).context, "", "an unnamed form says nothing");
    assert_eq!(at(&view, 4).context, r#"in region "Results""#);
}

#[test]
fn a_node_under_no_landmark_and_no_entry_has_no_context() {
    let page = tree(vec![
        node(900, "RootWebArea", "", &[1]),
        node(1, "button", "Alone", &[]),
    ]);
    let view = compact(&page, Mode::Interactive);

    assert_eq!(at(&view, 1).context, "");
    assert_eq!(at(&view, 1).landmark, "");
}

#[test]
fn an_entrys_name_is_cut_so_a_long_row_does_not_fill_an_answer() {
    let long = "word ".repeat(40);
    let page = tree(vec![
        node(900, "RootWebArea", "", &[1]),
        node(1, "row", &long, &[2]),
        node(2, "button", "Open", &[]),
    ]);
    let view = compact(&page, Mode::Interactive);

    let context = &at(&view, 2).context;
    assert!(context.chars().count() < 80, "{context}");
    assert!(context.contains('…'), "{context}");
}
