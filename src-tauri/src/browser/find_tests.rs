use super::*;
use crate::browser::ax::AxTree;
use crate::browser::test_support::{ax_fixture, buttons};
use crate::browser::view::{compact, Mode};

fn view_of(checked: &str) -> View {
    let mut fixture = ax_fixture();
    for node in fixture["nodes"]
        .as_array_mut()
        .expect("the fixture has nodes")
    {
        if node["role"]["value"] == "checkbox" {
            node["properties"] = serde_json::json!([
                { "name": "checked", "value": { "value": checked } }
            ]);
        }
    }
    let tree: AxTree = serde_json::from_value(fixture).expect("the fixture parses");
    compact(&tree, Mode::Interactive)
}

#[test]
fn the_name_is_what_a_query_is_looking_for() {
    let view = view_of("false");

    let hits = find(&view, "remember");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reference, "e14");
    assert_eq!(hits[0].role, "checkbox");
    assert_eq!(hits[0].name, "Remember me");
    // The context is what tells this control from another of the same name.
    assert_eq!(hits[0].context, r#"form "Sign in""#);
}

#[test]
fn a_query_is_not_case_sensitive_and_surrounding_spaces_do_not_matter() {
    let view = view_of("false");

    assert_eq!(find(&view, "  REMEMBER ME  ")[0].reference, "e14");
}

#[test]
fn a_role_is_a_way_to_ask_and_the_whole_of_that_role_answers() {
    let view = view_of("false");

    let hits = find(&view, "textbox");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reference, "e13", "the field is the only textbox");
}

#[test]
fn a_control_with_no_name_is_found_by_the_words_around_it() {
    let view = view_of("false");

    // The icon button has no name of its own; the paragraph above it is what
    // answers for it.
    let hits = find(&view, "forgot your password");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reference, "e17");
    assert_eq!(hits[0].name, "", "and it is the control with no name");

    // The words above a checkbox answer for the checkbox, and a control that
    // is one node further down shares those words: the query says a place, not
    // a line, and the first in document order is what it means.
    let above = find(&view, "keep me signed in");
    assert_eq!(above[0].reference, "e14");
}

#[test]
fn a_field_is_found_by_the_value_it_holds() {
    let view = view_of("false");

    let hits = find(&view, "person@example.test");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reference, "e13");
}

#[test]
fn a_query_nothing_says_matches_nothing_rather_than_everything() {
    let view = view_of("true");

    assert!(find(&view, "quantum entanglement").is_empty());
    assert!(
        find(&view, "   ").is_empty(),
        "an empty query is not a match"
    );
}

#[test]
fn the_best_match_comes_first_and_document_order_breaks_a_tie() {
    let view = view_of("false");

    // Three nodes are named "Sign in" and all three are exact matches, so
    // the words around them decide: the form and the button sit in a place
    // that says it too, the heading does not.
    let hits = find(&view, "sign in");
    let refs: Vec<&str> = hits.iter().map(|hit| hit.reference.as_str()).collect();
    assert_eq!(refs[..3], ["e19", "e15", "e11"]);
    assert_eq!(
        refs.len(),
        6,
        "and every control in the form is a weaker match, on the name of the          form they are in"
    );
}

#[test]
fn a_query_answers_at_most_twenty_matches() {
    let view = compact(&buttons(50, "Save"), Mode::Interactive);

    let hits = find(&view, "save");
    assert_eq!(hits.len(), MAX_MATCHES);
    // Every one of them is the same score, so the answer is the first twenty
    // in the order the page has them.
    assert_eq!(hits[0].reference, "e1");
    assert_eq!(hits[MAX_MATCHES - 1].reference, "e20");
}
