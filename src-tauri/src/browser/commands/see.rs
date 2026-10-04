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
use super::super::find;
use super::super::find_query::Query;
use super::super::registry::TabInfo;
use super::super::view::{self, Mode, View, FIELD_ROLES, VIEW_BUDGET};
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
        None => view::compact(&tree, mode),
        // A scope the page no longer has is a dead ref, not the whole page:
        // answering with everything would read as "here is your subtree" and
        // hide the one thing the caller has to re-read. It is looked for in
        // the tree, so a ref from a full view is live in an interactive one.
        Some(scope) => {
            view::compact_under(&tree, mode, node_of(scope)?).ok_or_else(|| stale_ref(scope))?
        }
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
        read_field_markup(page, &mut view).await;
    }
    let matches: Vec<Value> = find::rank(&view, &query)
        .into_iter()
        .map(|hit| {
            json!({
                "ref": hit.reference,
                "role": hit.role,
                "name": hit.name,
                "context": hit.context,
            })
        })
        .collect();
    Ok(json!({ "matches": matches }))
}

/// Read what the markup of the page's fields says about them. The accessible
/// tree has no `type=search`, no `name` and no `id`, and a search field with no
/// accessible name is told from the page's other fields by exactly those.
///
/// A field whose markup cannot be read is a field with no hints, never a
/// failed `find`: the answer is only less sure of itself.
async fn read_field_markup(page: &dyn Page, view: &mut View) {
    for node in view
        .nodes
        .iter_mut()
        .filter(|node| FIELD_ROLES.contains(&node.role.as_str()))
        .take(FIELDS_READ)
    {
        let Ok(described) = page
            .call(
                "DOM.describeNode",
                json!({ "backendNodeId": node.backend_id, "depth": 0 }),
            )
            .await
        else {
            continue;
        };
        node.hints = attribute_values(&described, &FIELD_ATTRIBUTES);
    }
}

/// The values of the named attributes, space-joined. The runtime sends a node's
/// attributes as one flat list, `[name, value, name, value, ...]`.
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
        .filter_map(|pair| pair[1].as_str())
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
