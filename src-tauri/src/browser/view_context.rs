//! Where a node sits on the page: the landmark it is under and the row or list
//! item it belongs to. `find` tells two controls with one name apart by this,
//! and ranks a control in the page's navigation ahead of one lost in its body.
//!
//! Read off every ancestor the walk passes, kept or not: an unnamed `navigation`
//! is not a line of the view, and it is still the part of the page a link is in.

use std::collections::HashMap;
use std::rc::Rc;

use super::ax::{AxNode, AxTree};
use super::view_line::clip;

/// Roles that say which part of the page this is.
const LANDMARKS: [&str; 10] = [
    "banner",
    "navigation",
    "main",
    "complementary",
    "contentinfo",
    "search",
    "dialog",
    "alertdialog",
    "form",
    "region",
];

/// The landmarks that are only landmarks when the page named them.
const NAMED_ONLY: [&str; 2] = ["form", "region"];

/// Roles that are one entry of a repeated structure.
const ITEMS: [&str; 4] = ["row", "listitem", "article", "treeitem"];

/// The longest name an entry carries, and the most nodes read to find one.
const LABEL_MAX: usize = 60;
const TEXT_NODES: usize = 200;

/// The tree indexed by the `nodeId` each child list names.
pub type Index<'a> = HashMap<&'a str, usize>;

/// One ancestor worth naming.
struct Entry {
    role: String,
    label: String,
}

impl Entry {
    fn new(role: &str, name: &str) -> Self {
        let label = if name.is_empty() {
            role.to_owned()
        } else {
            format!("{role} \"{}\"", clip(name, LABEL_MAX))
        };
        Entry {
            role: role.to_owned(),
            label,
        }
    }
}

/// The landmark and the item a node is inside, nearest first. Cloned down the
/// walk, so it holds its entries behind `Rc`: most nodes add nothing to it.
#[derive(Clone, Default)]
pub struct Ancestry {
    landmark: Option<Rc<Entry>>,
    item: Option<Rc<Entry>>,
}

impl Ancestry {
    /// What the children of `node` sit in, or None when `node` adds nothing to
    /// what its own parent already gave them.
    pub fn within(&self, node: &AxNode, tree: &AxTree, index: &Index<'_>) -> Option<Ancestry> {
        let role = node.role();
        let name = node.name();
        if LANDMARKS.contains(&role.as_str())
            && !(NAMED_ONLY.contains(&role.as_str()) && name.is_empty())
        {
            return Some(Ancestry {
                landmark: Some(Rc::new(Entry::new(&role, &name))),
                item: self.item.clone(),
            });
        }
        if ITEMS.contains(&role.as_str()) {
            // An entry the page did not name is still named by what it says.
            let label = if name.is_empty() {
                text_under(tree, index, node)
            } else {
                name
            };
            if label.is_empty() {
                return None;
            }
            return Some(Ancestry {
                landmark: self.landmark.clone(),
                item: Some(Rc::new(Entry::new(&role, &label))),
            });
        }
        None
    }

    /// `in row "1. A story", in navigation "top bar"`: the entry first, because
    /// it is the one that tells two same-named controls apart.
    pub fn context(&self) -> String {
        [&self.item, &self.landmark]
            .into_iter()
            .flatten()
            .map(|entry| format!("in {}", entry.label))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The role of the landmark, or empty when the node is under none.
    pub fn landmark(&self) -> &str {
        self.landmark
            .as_ref()
            .map(|entry| entry.role.as_str())
            .unwrap_or_default()
    }
}

/// The page's own words under a node, joined, for an entry that has no name:
/// a table row is named by its cells.
fn text_under(tree: &AxTree, index: &Index<'_>, node: &AxNode) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut length = 0;
    let mut pending: Vec<&AxNode> = vec![node];
    let mut visited = 0;
    while let Some(next) = pending.pop() {
        visited += 1;
        if visited > TEXT_NODES || length >= LABEL_MAX {
            break;
        }
        if next.role() == "StaticText" && !next.name().trim().is_empty() {
            let text = next.name().trim().to_owned();
            length += text.chars().count() + 1;
            words.push(text);
        }
        // Reversed, so the stack is read in document order.
        pending.extend(
            next.child_ids
                .iter()
                .rev()
                .filter_map(|child| index.get(child.as_str()))
                .filter_map(|at| tree.nodes.get(*at)),
        );
    }
    words.join(" ")
}

#[cfg(test)]
#[path = "view_context_tests.rs"]
mod tests;
