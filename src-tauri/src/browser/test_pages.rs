//! Whole pages for `find` to be asked about, laid out the way the runtime lays
//! out the real thing: landmarks with no name, a table of rows, a search field
//! with no accessible name. Written by hand and about this app's own
//! questions; no site's content is copied into them.

#![cfg(test)]

use serde_json::{json, Value};

use super::ax::AxTree;
use super::test_support::{ax_node, ax_property, ax_with_property};

/// One node, its `nodeId` being its `backendDOMNodeId` written out.
pub fn node(backend: u64, role: &str, name: &str, children: &[u64]) -> Value {
    let ids: Vec<String> = children.iter().map(u64::to_string).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    ax_node(&backend.to_string(), backend, role, name, &refs)
}

pub fn tree(nodes: Vec<Value>) -> AxTree {
    serde_json::from_value(json!({ "nodes": nodes })).expect("the fixture parses as a tree")
}

/// A reference page: a banner holding a search landmark with a field that has
/// no accessible name and a "Search" button, a navigation of links, and a
/// footer whose "Help" link shares its name with one in the navigation. The
/// footer comes first in the document on purpose, so that reading order alone
/// would put it first.
pub fn encyclopedia() -> AxTree {
    tree(vec![
        node(900, "RootWebArea", "Encyclopedia", &[901, 902, 907]),
        node(901, "contentinfo", "", &[910]),
        node(910, "link", "Help", &[]),
        node(902, "banner", "", &[903, 904]),
        node(903, "link", "Main page", &[]),
        node(904, "search", "", &[905, 906]),
        ax_with_property(
            node(905, "combobox", "", &[]),
            ax_property("expanded", "booleanOrUndefined", json!(false)),
        ),
        node(906, "button", "Search", &[]),
        node(907, "navigation", "Contents", &[908, 909]),
        node(908, "link", "History", &[]),
        node(909, "link", "Help", &[]),
    ])
}

/// A report: a title and a subheading, three links, two buttons and a field.
pub fn report() -> AxTree {
    let heading = |backend, name: &str, level: u64| {
        ax_with_property(
            node(backend, "heading", name, &[]),
            ax_property("level", "integer", json!(level)),
        )
    };
    tree(vec![
        node(
            900,
            "RootWebArea",
            "Quarterly report",
            &[1, 2, 3, 4, 5, 6, 7, 8],
        ),
        heading(1, "Quarterly report", 1),
        heading(2, "Totals", 2),
        node(3, "link", "Home", &[]),
        node(4, "link", "Archive", &[]),
        node(5, "link", "Next", &[]),
        node(6, "button", "Download", &[]),
        node(7, "button", "Share", &[]),
        node(8, "textbox", "Filter", &[]),
    ])
}

/// A front page: a top bar of links, then a table with one row per story, each
/// row holding its number, its title link and its comments link. The rows have
/// no accessible name, so what names them is what they say.
pub fn front_page() -> AxTree {
    let mut nodes = vec![
        node(900, "RootWebArea", "Front page", &[901, 902]),
        node(901, "navigation", "top bar", &[903, 904]),
        node(903, "link", "new", &[]),
        node(904, "link", "login", &[]),
        node(902, "table", "", &[910, 920, 930]),
    ];
    for (row, number, title, comments) in [
        (910, "1.", "First story", "12 comments"),
        (920, "2.", "Second story", "3 comments"),
        (930, "3.", "Third story", "7 comments"),
    ] {
        nodes.extend(story_row(row, number, title, comments));
    }
    tree(nodes)
}

/// A news front page whose top bar is a bare container, not a landmark: a
/// cell of three links, then `stories` rows, each with
/// its number, its title's link, its age and a "past" link. The top bar's own
/// "past" is `e905`; the rows start at `e1000`, ten ids apiece.
pub fn news(stories: u64) -> AxTree {
    let mut nodes = vec![
        node(900, "RootWebArea", "News", &[901]),
        node(902, "generic", "", &[903]),
        node(903, "cell", "", &[904, 905, 906]),
        node(904, "link", "new", &[907]),
        node(907, "StaticText", "new", &[]),
        node(905, "link", "past", &[908]),
        node(908, "StaticText", "past", &[]),
        node(906, "link", "comments", &[909]),
        node(909, "StaticText", "comments", &[]),
    ];
    let mut table = vec![902];
    for story in 1..=stories {
        let row = 1_000 + 10 * story;
        table.push(row);
        nodes.extend([
            node(row, "row", "", &[row + 1, row + 2, row + 3]),
            node(row + 1, "cell", "", &[row + 4]),
            node(row + 4, "StaticText", &format!("{story}."), &[]),
            node(row + 2, "cell", "", &[row + 5]),
            node(
                row + 5,
                "link",
                &format!("Story number {story}"),
                &[row + 6],
            ),
            node(row + 6, "StaticText", &format!("Story number {story}"), &[]),
            node(row + 3, "cell", "", &[row + 7, row + 8]),
            node(row + 7, "StaticText", "3 hours ago", &[]),
            node(row + 8, "link", "past", &[row + 9]),
            node(row + 9, "StaticText", "past", &[]),
        ]);
    }
    nodes.push(node(901, "table", "", &table));
    tree(nodes)
}

/// One table row: `row` holds three cells, and the cells hold a number, the
/// title's link and the comments' link, each link with the text it is made of.
fn story_row(row: u64, number: &str, title: &str, comments: &str) -> Vec<Value> {
    vec![
        node(row, "row", "", &[row + 1, row + 2, row + 3]),
        node(row + 1, "cell", "", &[row + 4]),
        node(row + 4, "StaticText", number, &[]),
        node(row + 2, "cell", "", &[row + 5]),
        node(row + 5, "link", title, &[row + 6]),
        node(row + 6, "StaticText", title, &[]),
        node(row + 3, "cell", "", &[row + 7]),
        node(row + 7, "link", comments, &[row + 8]),
        node(row + 8, "StaticText", comments, &[]),
    ]
}

/// Two fields, one of them with no accessible name and nothing around it: all
/// the page says about it is in its markup.
pub fn bare_fields() -> AxTree {
    tree(vec![
        node(900, "RootWebArea", "", &[1, 2]),
        node(1, "combobox", "", &[]),
        node(2, "textbox", "Email", &[]),
    ])
}
