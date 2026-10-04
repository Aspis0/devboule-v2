//! What an action that put input into a field answers with besides the field.
//!
//! In this order: the popups that opened (a listbox of suggestions, a menu, a
//! dialog), a few lines of the page's other changes with those near the field
//! first, and a count of what was left out. The field's own line is `target`,
//! which the delta carries ahead of all of this.

use serde::Serialize;

use super::delta::diff;
use super::view::{View, ViewNode};

/// How many lines of popup one answer carries.
const POPUP_LINES: usize = 8;

/// How many lines of everything else.
const OTHER_LINES: usize = 10;

/// Roles that are a popup, and the roles of what fills one.
const POPUP_ROLES: [&str; 4] = ["listbox", "menu", "dialog", "alertdialog"];
const POPUP_MEMBERS: [&str; 4] = ["option", "menuitem", "menuitemcheckbox", "menuitemradio"];

/// The part of a delta an input action keeps. An empty list is not sent: most
/// of the time nothing but the field changed.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Briefly {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub opened: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// How many lines were left out, popup and other together.
    #[serde(skip_serializing_if = "is_zero")]
    pub more: usize,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

/// What the action left worth reading, besides the field itself.
pub fn briefly(before: &View, after: &View, target: Option<u64>) -> Briefly {
    let mut found = diff(before, after);
    // The field's own line is `target`, and is not a change to read twice.
    found.changed.retain(|node| Some(node.backend_id) != target);
    let (popup, added): (Vec<&ViewNode>, Vec<&ViewNode>) =
        found.added.into_iter().partition(|node| is_popup(node));
    // The field the caller named, or the one that has the caret: what is near
    // it is what the action most likely did.
    let anchor = target
        .and_then(|id| after.nodes.iter().find(|node| node.backend_id == id))
        .or_else(|| after.focused());
    let near = |node: &ViewNode| anchor.is_some_and(|anchor| anchor.context == node.context);

    // Changes before additions before removals, and inside each the ones near
    // the field first; the sort is stable, so document order is what is left.
    let mut others: Vec<(u8, &ViewNode)> = found
        .changed
        .into_iter()
        .map(|node| (0, node))
        .chain(added.into_iter().map(|node| (1, node)))
        .chain(found.removed.into_iter().map(|node| (2, node)))
        .collect();
    others.sort_by_key(|(kind, node)| (!near(node), *kind));

    let mut briefly = Briefly {
        more: popup.len().saturating_sub(POPUP_LINES) + others.len().saturating_sub(OTHER_LINES),
        opened: popup
            .iter()
            .take(POPUP_LINES)
            .map(|node| node.line.clone())
            .collect(),
        ..Briefly::default()
    };
    for (kind, node) in others.into_iter().take(OTHER_LINES) {
        let line = node.line.clone();
        match kind {
            0 => briefly.changed.push(line),
            1 => briefly.added.push(line),
            _ => briefly.removed.push(line),
        }
    }
    briefly
}

fn is_popup(node: &ViewNode) -> bool {
    POPUP_ROLES.contains(&node.role.as_str()) || POPUP_MEMBERS.contains(&node.role.as_str())
}

#[cfg(test)]
#[path = "delta_input_tests.rs"]
mod tests;
