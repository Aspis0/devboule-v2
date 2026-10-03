//! What one action did to the page, without the page.
//!
//! An action answers with a delta, never with a second full view: the caller
//! already holds the view it acted on, and what it cannot know is what
//! changed. The delta is that, keyed on the ref, so the caller can see that a
//! checkbox did not flip (it is in `target`, unchanged) without spending a
//! snapshot to find out.

use serde::Serialize;

use super::view::{View, ViewNode};

/// How many lines each list carries before the rest is counted instead.
pub const LIST_CAP: usize = 40;

/// One page's address and title, read before and after an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub url: String,
    pub title: Option<String>,
}

/// What changed between two views of one page.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Delta {
    pub navigated: bool,
    pub url: String,
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dialog: Option<Dialog>,
    pub added: Vec<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub added_more: usize,
    pub removed: Vec<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub removed_more: usize,
    pub changed: Vec<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub changed_more: usize,
    /// The node the command acted on, read again after the page settled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// A dialog the page put up, which is the one thing an action can cause that
/// no line of the view explains.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Dialog {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

/// Compare the view before an action with the one after it.
///
/// `target` is passed in rather than looked up: only the command knows which
/// node it acted on, and a node the action removed is exactly the case where a
/// target line is worth having.
pub fn between(
    before: &View,
    after: &View,
    from: &Place,
    to: &Place,
    target: Option<u64>,
) -> Delta {
    let previous: std::collections::HashMap<u64, &ViewNode> = before
        .nodes
        .iter()
        .map(|node| (node.backend_id, node))
        .collect();
    let mut added = Vec::new();
    let mut changed = Vec::new();
    for node in &after.nodes {
        match previous.get(&node.backend_id) {
            None => added.push(node.line.clone()),
            Some(before) if before.line != node.line => changed.push(node.line.clone()),
            Some(_) => {}
        }
    }
    let live: std::collections::HashSet<u64> =
        after.nodes.iter().map(|node| node.backend_id).collect();
    let removed: Vec<String> = before
        .nodes
        .iter()
        .filter(|node| !live.contains(&node.backend_id))
        .map(|node| node.line.clone())
        .collect();
    Delta {
        navigated: from != to,
        url: to.url.clone(),
        title: to.title.clone(),
        focused: after.focused().map(|node| node.line.clone()),
        dialog: after
            .dialog()
            .map(|(kind, message)| Dialog { kind, message }),
        added: take(&added),
        added_more: added.len().saturating_sub(LIST_CAP),
        removed: take(&removed),
        removed_more: removed.len().saturating_sub(LIST_CAP),
        changed: take(&changed),
        changed_more: changed.len().saturating_sub(LIST_CAP),
        target: target.and_then(|id| after.line_of(id).map(str::to_owned)),
    }
}

/// The first `LIST_CAP` lines of a list.
fn take(lines: &[String]) -> Vec<String> {
    lines.iter().take(LIST_CAP).cloned().collect()
}

#[cfg(test)]
#[path = "delta_tests.rs"]
mod tests;
