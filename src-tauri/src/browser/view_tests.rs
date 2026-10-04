use super::*;
use crate::browser::ax::AxTree;
use crate::browser::test_support::{ax_fixture, ax_node};
use crate::browser::view::Mode;
use crate::browser::view_walk::compact;

fn fixture() -> AxTree {
    serde_json::from_value(ax_fixture()).expect("the fixture parses as a tree")
}

#[test]
fn a_view_is_cut_at_its_budget_and_the_cursor_is_the_node_that_did_not_fit() {
    let tree: AxTree = serde_json::from_value(serde_json::json!({
        "nodes": [
            ax_node("1", 1, "form", "A long form", &["2", "3", "4"]),
            ax_node("2", 2, "button", "One", &[]),
            ax_node("3", 3, "button", "Two", &[]),
            ax_node("4", 4, "button", "Three", &[]),
        ]
    }))
    .expect("the fixture parses");
    let view = compact(&tree, Mode::Interactive);

    // A budget of exactly the first three lines, so what does not fit is the
    // fourth whatever the lines happen to be long.
    let head: usize = view.nodes[..3].iter().map(|node| node.line.len() + 1).sum();
    let first = view.slice(0, head);
    assert!(first.truncated, "three lines do not fit four");
    assert_eq!(
        first.cursor,
        Some("e4".to_owned()),
        "the cursor names the first node that did not fit"
    );
    assert!(!first.text.contains("Three"));

    let rest = view.slice(3, VIEW_BUDGET);
    assert_eq!(rest.text, "  - button \"Three\" [ref=e4]");
    assert!(!rest.truncated);
    assert_eq!(rest.cursor, None);
}

#[test]
fn a_ref_is_an_e_and_a_node_id_and_nothing_else() {
    assert_eq!(parse_ref("e1234"), Some(1234));
    assert_eq!(parse_ref("e0"), Some(0));
    // A name that is not a ref is a dead ref, never some other node's.
    assert_eq!(parse_ref("1234"), None);
    assert_eq!(parse_ref("button"), None);
    assert_eq!(parse_ref("e"), None);
    assert_eq!(parse_ref("e12x"), None);
    assert_eq!(parse_ref(""), None);
    assert_eq!(parse_ref("e99999999999999999999999"), None);
}

#[test]
fn a_focus_and_a_dialog_are_read_off_the_view_rather_than_the_page() {
    let view = compact(&fixture(), Mode::Interactive);

    let focused = view.focused().expect("the fixture focuses the field");
    assert_eq!(focused.backend_id, 13);
    assert_eq!(view.dialog(), None);
    assert!(!focused.disabled, "the fixture's field is not disabled");
}
