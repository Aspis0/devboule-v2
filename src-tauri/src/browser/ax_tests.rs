use super::*;
use crate::browser::test_support::{ax_fixture, FakePage};

fn tree(json: serde_json::Value) -> AxTree {
    serde_json::from_value(json).expect("the fixture parses as a tree")
}

#[test]
fn the_tree_is_read_out_of_the_answers_the_runtime_gives() {
    let parsed = tree(ax_fixture());

    let form = parsed
        .nodes
        .iter()
        .find(|node| node.role() == "form")
        .expect("the fixture has a form");
    assert_eq!(form.name(), "Sign in");
    assert_eq!(form.backend_dom_node_id, Some(19));
    // The tree is linked by the protocol's own spelling of the two ids, and a
    // mistyped rename here leaves a tree that parses and has no children.
    assert_eq!(form.node_id, "3");
    assert_eq!(form.child_ids, vec!["4", "5", "6", "7", "8", "9"]);

    // Every field is read as text whatever it arrived as: `level` is a number
    // and `checked` is the string "false".
    let heading = parsed
        .nodes
        .iter()
        .find(|node| node.role() == "heading")
        .expect("the fixture has a heading");
    assert_eq!(heading.property("level"), "2");
    let checkbox = parsed
        .nodes
        .iter()
        .find(|node| node.role() == "checkbox")
        .expect("the fixture has a checkbox");
    assert_eq!(checkbox.property("checked"), "false");
    assert!(checkbox.property("expanded").is_empty());

    // An ignored node is a node the runtime computed for the layout and
    // nothing else.
    let ignored = parsed
        .nodes
        .iter()
        .find(|node| node.ignored)
        .expect("the fixture has an ignored node");
    assert_eq!(ignored.backend_dom_node_id, Some(18));
}

/// The parser reads only `value`, so a fixture that dropped `type` or `ignored`
/// would still parse and prove nothing about the shape the runtime sends.
#[test]
fn the_fixture_carries_every_field_the_protocol_requires_of_a_node() {
    let fixture = ax_fixture();
    for node in fixture["nodes"].as_array().expect("a list of nodes") {
        for field in ["nodeId", "backendDOMNodeId", "ignored", "role", "childIds"] {
            assert!(node.get(field).is_some(), "{field} is required: {node}");
        }
        let typed = ["role", "name", "value"]
            .into_iter()
            .filter_map(|field| node.get(field))
            .chain(
                node["properties"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|property| &property["value"]),
            );
        for value in typed {
            assert!(
                value["type"].is_string() && value.get("value").is_some(),
                "an AXValue is a type and a value: {value}"
            );
        }
    }
}

#[test]
fn a_tree_is_fetched_with_the_one_call_that_computes_it() {
    tauri::async_runtime::block_on(async {
        let page = FakePage::new().answering("Accessibility.getFullAXTree", ax_fixture());
        let fetched = super::tree(&page).await.expect("the page answers");
        assert_eq!(fetched.nodes.len(), 10);
        assert_eq!(page.called("Accessibility.getFullAXTree"), 1);
    });
}

#[test]
fn a_tree_the_runtime_will_not_send_is_a_refusal_and_not_an_empty_page() {
    tauri::async_runtime::block_on(async {
        let page = FakePage::new().answering(
            "Accessibility.getFullAXTree",
            serde_json::json!({ "nodes": "not a list" }),
        );
        let error = super::tree(&page)
            .await
            .expect_err("a bad tree is a failure");
        assert!(
            error.message().contains("accessibility tree"),
            "the failure names what the page would not give: {}",
            error.message()
        );
    });
}
