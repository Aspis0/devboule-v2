//! What one action did to the page, without the page.
//!
//! An action answers with a delta, never with a second full view: the caller
//! already holds the view it acted on, and what it cannot know is what
//! changed. The delta is that, keyed on the ref, so the caller can see that a
//! checkbox did not flip (it is in `target`, unchanged) without spending a
//! snapshot to find out.
//!
//! An action that left the page for another has no before to compare with:
//! every node of the new page would be "added", and two hundred lines of that
//! say less than the page's title and what it is made of. A navigation answers
//! with that summary instead.

use serde::Serialize;

use super::delta_input::{self, Briefly};
use super::view::{View, ViewNode, FIELD_ROLES};
use super::view_line::clip;

/// How many lines each list carries before the rest is counted instead.
pub const LIST_CAP: usize = 40;

/// How many headings a page summary names, and how much of each it reads.
const SUMMARY_HEADINGS: usize = 5;
const HEADING_MAX: usize = 80;

/// One page's address and title, and how many documents it has loaded, read
/// before and after an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub url: String,
    pub title: Option<String>,
    /// How many documents the tab's own frame has committed. The address can
    /// move without it moving (`pushState`, a hash), and it can move without
    /// the address (a reload).
    pub document: u64,
}

impl Place {
    /// Whether the action took the tab somewhere: to another address, or to a
    /// new document at the same one. A title that changed on its own is a change
    /// on the page, and is read as one.
    fn navigated_to(&self, to: &Place) -> bool {
        self.url != to.url || self.document != to.document
    }
}

/// What changed between two views of one page.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Delta {
    pub navigated: bool,
    /// The address changed and no document was loaded: the page moved itself,
    /// as a single-page app does. The summary describes where it is now.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub same_document: bool,
    pub url: String,
    pub title: Option<String>,
    /// The node the command acted on, read again after the page settled: the
    /// value or state that was verified, and so the first thing a caller reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dialog: Option<Dialog>,
    /// What came, went and differed inside the same document. A navigation has
    /// none: the page it left has nothing to be compared with.
    #[serde(flatten)]
    pub changes: Option<Lists>,
    /// What the page the action landed on is made of; only a navigation has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
}

/// The changes inside one document, in the shape the action calls for.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Lists {
    /// Everything that came, went and differed, each list capped.
    Everything(Changes),
    /// What an action that put input into a field leaves worth reading.
    Input(Briefly),
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

/// The lines of one document that came, went and differ, each list capped.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub added: Vec<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub added_more: usize,
    pub removed: Vec<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub removed_more: usize,
    pub changed: Vec<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub changed_more: usize,
}

/// A page in a few words: what it calls itself and how much there is to do.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Summary {
    /// The first headings, as `h1 "Title"`.
    pub headings: Vec<String>,
    pub links: usize,
    pub buttons: usize,
    pub fields: usize,
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
    delta_of(before, after, from, to, target, |before, after| {
        Lists::Everything(changes(before, after))
    })
}

/// The same for an action that put input into a field: the field's own
/// value first, then what popped up around it, then a few of the rest. The
/// page's other changes — a typed letter re-renders its sidebar, its radios and
/// its links — would otherwise bury the one line that says the field took it.
pub fn between_input(
    before: &View,
    after: &View,
    from: &Place,
    to: &Place,
    target: Option<u64>,
) -> Delta {
    delta_of(before, after, from, to, target, |before, after| {
        Lists::Input(delta_input::briefly(before, after, target))
    })
}

fn delta_of(
    before: &View,
    after: &View,
    from: &Place,
    to: &Place,
    target: Option<u64>,
    within_one_document: impl FnOnce(&View, &View) -> Lists,
) -> Delta {
    let navigated = from.navigated_to(to);
    let target_line = target.and_then(|id| after.line_of(id).map(str::to_owned));
    Delta {
        navigated,
        same_document: navigated && from.document == to.document,
        url: to.url.clone(),
        title: to.title.clone(),
        // The focused node is what the caller acted on more often than not, and
        // saying so twice would only be the second line to read past.
        focused: after
            .focused()
            .map(|node| node.line.clone())
            .filter(|line| Some(line) != target_line.as_ref()),
        target: target_line,
        dialog: after
            .dialog()
            .map(|(kind, message)| Dialog { kind, message }),
        changes: (!navigated).then(|| within_one_document(before, after)),
        summary: navigated.then(|| summarize(after)),
    }
}

/// The nodes that came, the nodes that differ, and the nodes that went, in
/// that order, each in document order.
pub(super) struct Diff<'a> {
    pub added: Vec<&'a ViewNode>,
    pub changed: Vec<&'a ViewNode>,
    pub removed: Vec<&'a ViewNode>,
}

pub(super) fn diff<'a>(before: &'a View, after: &'a View) -> Diff<'a> {
    let previous: std::collections::HashMap<u64, &ViewNode> = before
        .nodes
        .iter()
        .map(|node| (node.backend_id, node))
        .collect();
    let mut added = Vec::new();
    let mut changed = Vec::new();
    for node in &after.nodes {
        match previous.get(&node.backend_id) {
            None => added.push(node),
            Some(before) if before.line != node.line => changed.push(node),
            Some(_) => {}
        }
    }
    let live: std::collections::HashSet<u64> =
        after.nodes.iter().map(|node| node.backend_id).collect();
    let removed = before
        .nodes
        .iter()
        .filter(|node| !live.contains(&node.backend_id))
        .collect();
    Diff {
        added,
        changed,
        removed,
    }
}

fn changes(before: &View, after: &View) -> Changes {
    let found = diff(before, after);
    let lines = |nodes: &[&ViewNode]| -> Vec<String> {
        nodes.iter().map(|node| node.line.clone()).collect()
    };
    let (added, removed, changed) = (
        lines(&found.added),
        lines(&found.removed),
        lines(&found.changed),
    );
    Changes {
        added_more: added.len().saturating_sub(LIST_CAP),
        added: take(&added),
        removed_more: removed.len().saturating_sub(LIST_CAP),
        removed: take(&removed),
        changed_more: changed.len().saturating_sub(LIST_CAP),
        changed: take(&changed),
    }
}

/// What a view is made of, in the few words an agent decides its next step by.
fn summarize(view: &View) -> Summary {
    let headings = view
        .nodes
        .iter()
        .filter(|node| node.role == "heading" && !node.name.is_empty())
        .take(SUMMARY_HEADINGS)
        .map(|node| match node.level {
            Some(level) => format!("h{level} \"{}\"", clip(&node.name, HEADING_MAX)),
            None => format!("heading \"{}\"", clip(&node.name, HEADING_MAX)),
        })
        .collect();
    let count = |wanted: &dyn Fn(&str) -> bool| {
        view.nodes
            .iter()
            .filter(|node| wanted(node.role.as_str()))
            .count()
    };
    Summary {
        headings,
        links: count(&|role| role == "link"),
        buttons: count(&|role| role == "button"),
        fields: count(&|role| FIELD_ROLES.contains(&role)),
    }
}

/// The first `LIST_CAP` lines of a list.
fn take(lines: &[String]) -> Vec<String> {
    lines.iter().take(LIST_CAP).cloned().collect()
}

#[cfg(test)]
#[path = "delta_tests.rs"]
mod tests;
