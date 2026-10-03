//! The view of a page an agent reads: one line per node, indented by depth,
//! with the role, the name, the states and the ref.
//!
//! The compaction happens here, in the process that fetched the tree, and the
//! result is what crosses into the answer: on the pages measured in
//! `scout/browser-tabs/SPIKE-REPORT-cdp.md` this is 0.5-2.6 % of the tree, and
//! the tree itself is up to 3.6 MB of JSON that must never enter a frame.
//!
//! Which roles are kept is the spike's measured pair: an interactive node is
//! kept even with no name (an icon button is often the only route to a
//! control), a context node only with one, and everything else — `ignored`,
//! `generic`, `presentation`, every other role — is dropped.

use std::collections::{HashMap, HashSet};

use super::ax::{AxNode, AxTree, AxValue};

/// The characters a view may carry before it is cut and a cursor offered.
pub const VIEW_BUDGET: usize = 12_000;

/// The deepest indent a line is written at. A page can nest a hundred
/// containers deep and every one of them is a `generic` this drops.
const MAX_DEPTH: usize = 6;

/// The longest value a state carries.
const VALUE_MAX: usize = 80;

/// Roles a command can act on. Kept whole, unnamed ones included: an icon
/// button is usually the only route to the control it stands for.
const INTERACTIVE: [&str; 19] = [
    "button",
    "checkbox",
    "columnheader",
    "combobox",
    "gridcell",
    "link",
    "listbox",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "progressbar",
    "radio",
    "rowheader",
    "searchbox",
    "slider",
    "switch",
    "tab",
    "textbox",
];

/// Roles worth reading as the way to a node: a heading that says what the
/// region below it is, a landmark that says which part of the page this is.
const CONTEXT: [&str; 20] = [
    "alert",
    "alertdialog",
    "article",
    "banner",
    "blockquote",
    "caption",
    "code",
    "complementary",
    "contentinfo",
    "dialog",
    "figure",
    "form",
    "heading",
    "img",
    "list",
    "listitem",
    "main",
    "navigation",
    "region",
    "search",
];

/// Roles whose text is the page's own words, used as the `nearby` a `find`
/// scores against.
const TEXT_ROLES: [&str; 4] = ["StaticText", "paragraph", "heading", "listitem"];

/// What the view shows of one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Interactive nodes, plus the headings and landmarks around them.
    Interactive,
    /// Every node the runtime computed a role for.
    Full,
}

/// One line of a view: what it says, and the node it stands for.
#[derive(Debug, Clone)]
pub struct ViewNode {
    /// The `backendDOMNodeId` a ref names. Stable across a re-render, dead on
    /// the next navigation.
    pub backend_id: u64,
    pub role: String,
    pub name: String,
    pub value: String,
    pub description: String,
    pub placeholder: String,
    /// The last piece of text the node's own markup put above it, for `find`.
    pub nearby: String,
    /// The nearest named node above this one, as `role "name"`. This is what
    /// two controls with the same name are told apart by.
    pub context: String,
    pub focused: bool,
    pub disabled: bool,
    /// `Some` when the runtime reports a checked state, `None` when the node
    /// has none: a checkbox that did not flip and a control that cannot flip
    /// are different answers.
    pub checked: Option<bool>,
    pub depth: usize,
    pub line: String,
}

impl ViewNode {
    pub fn ref_text(&self) -> String {
        format!("e{}", self.backend_id)
    }

    /// The node as a `find` match names it back: role and name, no states.
    pub fn label(&self) -> String {
        if self.name.is_empty() {
            self.role.clone()
        } else {
            format!("{} \"{}\"", self.role, self.name)
        }
    }
}

/// A page's kept nodes, in document order.
#[derive(Debug, Clone, Default)]
pub struct View {
    pub nodes: Vec<ViewNode>,
}

/// One slice of a view: the text, whether more is left, and the ref that
/// continues it.
#[derive(Debug, Clone)]
pub struct Slice {
    pub text: String,
    pub truncated: bool,
    pub cursor: Option<String>,
}

impl View {
    pub fn line_of(&self, backend_id: u64) -> Option<&str> {
        self.nodes
            .iter()
            .find(|node| node.backend_id == backend_id)
            .map(|node| node.line.as_str())
    }

    pub fn focused(&self) -> Option<&ViewNode> {
        self.nodes.iter().find(|node| node.focused)
    }

    /// A dialog the page is showing, if it is showing one: the name of the
    /// dialog, which is what a page says in it.
    pub fn dialog(&self) -> Option<(String, String)> {
        self.nodes
            .iter()
            .find(|node| matches!(node.role.as_str(), "dialog" | "alertdialog"))
            .map(|node| (node.role.clone(), node.name.clone()))
    }

    /// `budget` characters from `from` on. The cursor is the ref of the first
    /// node that did not fit, and it is that node — not an offset — so a
    /// continuation survives a re-render of the page above it.
    pub fn slice(&self, from: usize, budget: usize) -> Slice {
        let mut text = String::new();
        let mut cursor = None;
        for node in self.nodes.iter().skip(from) {
            let line = node.line.clone();
            if !text.is_empty() && text.len() + line.len() + 1 > budget {
                cursor = Some(node.ref_text());
                break;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&line);
        }
        Slice {
            truncated: cursor.is_some(),
            cursor,
            text,
        }
    }
}

/// The ref a node is addressed by, or None. A ref is `e` and the node's
/// `backendDOMNodeId`; nothing else is a ref, so a typo is a missing node and
/// never some other node's.
pub fn parse_ref(text: &str) -> Option<u64> {
    text.strip_prefix('e')?.parse().ok()
}

/// Whether this mode keeps the node at all.
///
/// Interactive mode keeps a control even with no name, and a landmark or
/// heading only with one: `navigation ""` says nothing that the node below it
/// does not say better.
fn keeps(node: &AxNode, mode: Mode) -> bool {
    if node.ignored {
        return false;
    }
    let role = node.role();
    if role.is_empty() {
        return false;
    }
    match mode {
        Mode::Full => true,
        Mode::Interactive => {
            INTERACTIVE.contains(&role.as_str())
                || (CONTEXT.contains(&role.as_str()) && !node.name().is_empty())
        }
    }
}

/// The states a line carries, in a fixed order so two reads of one node are
/// byte-identical.
fn states(node: &AxNode) -> Vec<String> {
    let mut states = Vec::new();
    match node.property("checked").as_str() {
        "true" => states.push("checked".to_owned()),
        "false" => states.push("unchecked".to_owned()),
        _ => {}
    }
    match node.property("expanded").as_str() {
        "true" => states.push("expanded".to_owned()),
        "false" => states.push("collapsed".to_owned()),
        _ => {}
    }
    if node.property("selected") == "true" {
        states.push("selected".to_owned());
    }
    if node.property("disabled") == "true" {
        states.push("disabled".to_owned());
    }
    let value = node.value.as_ref().map(AxValue::text).unwrap_or_default();
    if !value.is_empty() {
        states.push(format!("value=\"{}\"", clip(&value, VALUE_MAX)));
    }
    let level = node.property("level");
    if !level.is_empty() {
        states.push(format!("level={level}"));
    }
    states
}

/// One node's line, as the view prints it and as a delta repeats it: the
/// indent, the marker, the role, the name, the states and the ref. Written
/// once, so a delta's lines are the view's lines.
fn line_for(node: &AxNode, backend_id: u64, depth: usize) -> String {
    let name = node.name();
    let mut line = format!("{}- {}", "  ".repeat(depth), node.role());
    if !name.is_empty() {
        line.push_str(&format!(" \"{name}\""));
    }
    for state in states(node) {
        line.push_str(&format!(" [{state}]"));
    }
    line.push_str(&format!(" [ref=e{backend_id}]"));
    line
}

/// Shorten to `max` characters, at a boundary that does not split one.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Compact a tree into the view a snapshot prints.
///
/// A context node is written once, where it is first reached: a page with four
/// `nav` landmarks would otherwise repeat the same line before every group it
/// contains, and the repeats would read as changes between two snapshots.
/// The tree indexed by the `nodeId` each child list names.
type Index<'a> = HashMap<&'a str, usize>;

pub fn compact(tree: &AxTree, mode: Mode) -> View {
    let index: Index<'_> = tree
        .nodes
        .iter()
        .enumerate()
        .map(|(position, node)| (node.node_id.as_str(), position))
        .collect();
    // A tree with no parent anywhere has nothing to start from, and the
    // runtime always sends one; the first node with children is the safe guess.
    let root = tree
        .nodes
        .iter()
        .position(|node| !node.child_ids.is_empty())
        .unwrap_or(0);
    let mut step = Walk {
        tree,
        index: &index,
        mode,
        written: HashSet::new(),
        view: View::default(),
    };
    walk(&mut step, &[root], 0, 0, None);
    step.view
}

/// What the walk carries: the tree it reads, the view it writes and the one
/// rule it applies to every node it passes.
struct Walk<'a> {
    tree: &'a AxTree,
    index: &'a Index<'a>,
    mode: Mode,
    /// The nodes already written. A node reachable twice is written once, so
    /// a delta between two reads sees a change and not a duplicate.
    written: HashSet<u64>,
    view: View,
}

fn walk(step: &mut Walk<'_>, siblings: &[usize], at: usize, depth: usize, context: Option<usize>) {
    let Walk {
        tree,
        index,
        mode,
        written,
        view,
    } = step;
    let Some(node) = tree.nodes.get(siblings[at]) else {
        return;
    };
    let Some(backend_id) = node.backend_dom_node_id else {
        return;
    };
    let keep = keeps(node, *mode) && !written.contains(&backend_id);
    let mut next_depth = depth;
    let mut next_context = context;
    if keep && !node.name().is_empty() {
        next_depth = (depth + 1).min(MAX_DEPTH);
        next_context = Some(view.nodes.len());
    }
    if keep {
        written.insert(backend_id);
        view.nodes.push(ViewNode {
            backend_id,
            role: node.role(),
            name: node.name(),
            value: node.text_of("value"),
            description: node.text_of("description"),
            placeholder: node.placeholder(),
            nearby: nearby_text(tree, index, siblings, at),
            context: context
                .map(|above| view.nodes[above].label())
                .unwrap_or_default(),
            focused: node.property("focused") == "true",
            disabled: node.property("disabled") == "true",
            checked: match node.property("checked").as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            depth,
            line: line_for(node, backend_id, depth),
        });
    }
    let children: Vec<usize> = node
        .child_ids
        .iter()
        .filter_map(|child| index.get(child.as_str()).copied())
        .collect();
    for position in 0..children.len() {
        walk(step, &children, position, next_depth, next_context);
    }
}

/// The text the page put just above this node, which is what tells two
/// same-named controls apart: the "Search" box in a header and the one in a
/// form are both `textbox "Search"`, and only the words above them differ.
///
/// Bounded to the three preceding siblings and to their own subtrees: this is
/// a reading aid for a query, not a text extraction.
fn nearby_text(tree: &AxTree, index: &Index<'_>, siblings: &[usize], at: usize) -> String {
    for previous in siblings[..at].iter().rev().take(3) {
        if let Some(text) = subtree_text(tree, index, *previous, 1) {
            return text;
        }
    }
    String::new()
}

/// The last text-bearing node in a subtree, or None if it holds none. Two
/// levels is as deep as a label and its input go together.
fn subtree_text(tree: &AxTree, index: &Index<'_>, position: usize, level: usize) -> Option<String> {
    let node = tree.nodes.get(position)?;
    let mut found = None;
    if level > 0 && TEXT_ROLES.contains(&node.role().as_str()) && !node.name().is_empty() {
        found = Some(node.name());
    }
    if level >= 2 {
        return found;
    }
    for child in &node.child_ids {
        if let Some(child) = index.get(child.as_str()).copied() {
            found = subtree_text(tree, index, child, level + 1).or(found);
        }
    }
    found
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
