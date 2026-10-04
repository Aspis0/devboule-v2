use super::*;
use crate::browser::ax::AxTree;
use crate::browser::test_support::{ax_fixture, ax_node, ax_text, ax_with};
use crate::browser::view::VIEW_BUDGET;
use crate::browser::view_line::VALUE_MAX;

fn fixture() -> AxTree {
    serde_json::from_value(ax_fixture()).expect("the fixture parses as a tree")
}

fn lines(view: &View) -> Vec<String> {
    view.slice(0, VIEW_BUDGET)
        .text
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn an_interactive_view_keeps_the_controls_and_the_landmarks_around_them() {
    let view = compact(&fixture(), Mode::Interactive);

    assert_eq!(
        lines(&view),
        vec![
            r#"- heading "Sign in" [level=2] [ref=e11]"#.to_owned(),
            r#"- form "Sign in" [ref=e19]"#.to_owned(),
            r#"  - textbox "Email" [value="person@example.test"] [ref=e13]"#.to_owned(),
            r#"  - checkbox "Remember me" [unchecked] [ref=e14]"#.to_owned(),
            r#"  - button "Sign in" [ref=e15]"#.to_owned(),
            r#"  - button [ref=e17]"#.to_owned(),
        ],
        "the form's children are indented under it, and the page's own words \
         (a paragraph, a StaticText) and its ignored nodes are not lines"
    );
}

#[test]
fn a_full_view_is_every_role_the_runtime_computed_and_nothing_ignored() {
    let view = compact(&fixture(), Mode::Full);

    let roles: Vec<&str> = view.nodes.iter().map(|node| node.role.as_str()).collect();
    assert!(
        roles.contains(&"RootWebArea"),
        "full keeps what interactive drops"
    );
    assert!(
        roles.contains(&"StaticText"),
        "full keeps the page's own words"
    );
    assert!(roles.contains(&"paragraph"));
    assert!(
        !view.nodes.iter().any(|node| node.backend_id == 18),
        "an ignored node is never a line in either mode"
    );
}

#[test]
fn a_control_with_no_name_is_still_a_line_because_it_is_the_way_to_something() {
    let view = compact(&fixture(), Mode::Interactive);
    let icon = view
        .nodes
        .iter()
        .find(|node| node.backend_id == 17)
        .expect("the icon button is kept");

    assert_eq!(icon.role, "button");
    assert!(icon.name.is_empty());
    assert_eq!(icon.line, "  - button [ref=e17]");
}

#[test]
fn the_states_a_line_carries_are_read_from_the_properties_in_a_fixed_order() {
    let view = compact(&fixture(), Mode::Interactive);
    let lines = lines(&view).join("\n");

    assert!(
        lines.contains(r#"[level=2]"#),
        "a heading says how deep it is"
    );
    assert!(lines.contains("[unchecked]"), "a checkbox says both ways");
    assert!(
        !lines.contains("[disabled]"),
        "a control that is not disabled does not say it is disabled"
    );
}

#[test]
fn a_value_is_clipped_because_a_field_can_hold_a_whole_document() {
    let long = "x".repeat(200);
    let tree: AxTree = serde_json::from_value(serde_json::json!({
        "nodes": [ax_with(
            ax_node("1", 5, "textbox", "Notes", &[]),
            "value",
            ax_text(&long),
        )]
    }))
    .expect("the fixture parses");

    let mut view = compact(&tree, Mode::Interactive);
    let line = view.nodes.remove(0);
    let printed = line
        .line
        .split("[value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the line carries a value");
    assert_eq!(
        printed.chars().count(),
        VALUE_MAX,
        "80 characters and an ellipsis, whatever the field holds: {printed}"
    );
    assert!(line.line.ends_with("[ref=e5]"));
    assert_eq!(
        line.value.chars().count(),
        200,
        "the node keeps what it really holds; only the line is short"
    );
}

#[test]
fn a_named_control_keeps_the_nearest_named_ancestor_as_its_context() {
    let view = compact(&fixture(), Mode::Interactive);
    let checkbox = view
        .nodes
        .iter()
        .find(|node| node.backend_id == 14)
        .expect("the checkbox is kept");

    assert_eq!(checkbox.context, r#"in form "Sign in""#);
    // And the words above it, which is what tells it from another control of
    // the same name.
    assert_eq!(checkbox.nearby, "Keep me signed in");
    assert_eq!(checkbox.checked, Some(false));
}
