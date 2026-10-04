//! `snapshot` and `find`: the two commands that read a page.
//!
//! Neither measures the page, so neither puts a parked one on screen: the
//! accessible tree of a parked page is byte-identical to the same page's
//! presented one (measured in the spike report), and overriding the metrics
//! would re-lay out a responsive document for no gain. Everything that DOES
//! measure goes through `act`, which overrides first.

use serde::Deserialize;
use serde_json::{json, Value};

use devboule_protocol::BrowserError;

use super::super::cdp::Page;
use super::super::find::{self, Match};
use super::super::find_query::Query;
use super::super::registry::TabInfo;
use super::super::view::{Mode, View, ViewNode, FIELD_ROLES, VIEW_BUDGET};
use super::super::view_walk;
use super::{args_of, host_error, node_of, stale_ref, tree_of, view_of};

/// The most fields whose markup one `find` reads. A page with more is a page
/// of forms, and the first twenty are what a question about "the field" means.
const FIELDS_READ: usize = 20;

/// The attributes of a field that say what it is for.
const FIELD_ATTRIBUTES: [&str; 4] = ["type", "name", "id", "placeholder"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotArgs {
    scope: Option<String>,
    mode: Option<String>,
    cursor: Option<String>,
}

#[derive(Deserialize)]
struct FindArgs {
    query: String,
}

pub async fn snapshot(tab: &TabInfo, page: &dyn Page, args: &Value) -> Result<Value, BrowserError> {
    let asked: SnapshotArgs = args_of(args)?;
    let mode = match asked.mode.as_deref() {
        None | Some("interactive") => Mode::Interactive,
        Some("full") => Mode::Full,
        Some(other) => return Err(host_error(format!("{other} is not a snapshot mode."))),
    };
    let tree = tree_of(page).await?;
    let scoped = match &asked.scope {
        None => view_walk::compact(&tree, mode),
        // A scope the page no longer has is a dead ref, not the whole page:
        // answering with everything would read as "here is your subtree" and
        // hide the one thing the caller has to re-read. It is looked for in
        // the tree, so a ref from a full view is live in an interactive one.
        Some(scope) => view_walk::compact_under(&tree, mode, node_of(scope)?)
            .ok_or_else(|| stale_ref(scope))?,
    };
    // A cursor the page no longer has — a re-render removed the node it named —
    // restarts at the top rather than silently skipping what is left. The
    // cursor is the first node that did not fit, so it is where this one starts.
    let from = match &asked.cursor {
        None => 0,
        Some(cursor) => position_of(&scoped, node_of(cursor)?).unwrap_or(0),
    };
    let slice = scoped.slice(from, VIEW_BUDGET);
    Ok(json!({
        "url": tab.url,
        "title": tab.title,
        "view": slice.text,
        "truncated": slice.truncated,
        "cursor": slice.cursor,
    }))
}

pub async fn find(_tab: &TabInfo, page: &dyn Page, args: &Value) -> Result<Value, BrowserError> {
    let asked: FindArgs = args_of(args)?;
    let query = Query::parse(&asked.query);
    let mut view = view_of(page, Mode::Interactive).await?;
    if query.asks_for_fields() {
        for node in fields_of(&mut view).take(FIELDS_READ) {
            read_field_markup(page, node).await;
        }
    }
    let mut hits = find::rank(&view, &query);
    describe_bare_fields(page, &mut view, &mut hits).await;
    let matches: Vec<Value> = hits
        .into_iter()
        .map(|hit| {
            let mut one = json!({
                "ref": hit.reference,
                "role": hit.role,
                "name": hit.name,
                "context": hit.context,
            });
            if !hit.detail.is_empty() {
                one["detail"] = json!(hit.detail);
            }
            one
        })
        .collect();
    Ok(json!({ "matches": matches }))
}

/// The nodes a person types into.
fn fields_of(view: &mut View) -> impl Iterator<Item = &mut ViewNode> {
    view.nodes
        .iter_mut()
        .filter(|node| FIELD_ROLES.contains(&node.role.as_str()))
}

/// Read what the markup of one field says about it. The accessible tree has no
/// `type=search`, no `name` and no `id`, and a search field with no accessible
/// name is told from the page's other fields by exactly those.
///
/// A field whose markup cannot be read is a field with no hints, never a
/// failed `find`: the answer is only less sure of itself.
async fn read_field_markup(page: &dyn Page, node: &mut ViewNode) {
    let Ok(described) = page
        .call(
            "DOM.describeNode",
            json!({ "backendNodeId": node.backend_id, "depth": 0 }),
        )
        .await
    else {
        return;
    };
    node.hints = attribute_values(&described, &FIELD_ATTRIBUTES);
}

/// Say what each nameless field among the matches is, reading its markup when
/// the question did not already: a caller handed a field with no name has
/// nothing else to confirm it picked the right one.
async fn describe_bare_fields(page: &dyn Page, view: &mut View, hits: &mut [Match]) {
    for hit in hits
        .iter_mut()
        .filter(|hit| hit.name.is_empty() && FIELD_ROLES.contains(&hit.role.as_str()))
    {
        let Some(node) = view
            .nodes
            .iter_mut()
            .find(|node| node.ref_text() == hit.reference)
        else {
            continue;
        };
        if node.hints.is_empty() {
            read_field_markup(page, node).await;
        }
        hit.detail = find::describe(node);
    }
}

/// The named attributes as `name=value`, space-joined, a value with a space in
/// it quoted. The runtime sends a node's attributes as one flat list,
/// `[name, value, name, value, ...]`.
fn attribute_values(described: &Value, wanted: &[&str]) -> String {
    let Some(attributes) = described
        .get("node")
        .and_then(|node| node.get("attributes"))
        .and_then(Value::as_array)
    else {
        return String::new();
    };
    attributes
        .as_chunks::<2>()
        .0
        .iter()
        .filter(|pair| pair[0].as_str().is_some_and(|name| wanted.contains(&name)))
        .filter_map(|pair| Some((pair[0].as_str()?, pair[1].as_str()?)))
        .filter(|(_, value)| !value.is_empty())
        .map(|(name, value)| {
            if value.contains(char::is_whitespace) {
                format!("{name}=\"{value}\"")
            } else {
                format!("{name}={value}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where a ref sits in a view, or None when the node is not on it.
fn position_of(view: &View, backend_id: u64) -> Option<usize> {
    view.nodes
        .iter()
        .position(|node| node.backend_id == backend_id)
}

#[cfg(test)]
#[path = "see_tests.rs"]
mod tests;
