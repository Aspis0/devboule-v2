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
use super::super::registry::TabInfo;
use super::super::view::{Mode, View, ViewNode, VIEW_BUDGET};
use super::{args_of, host_error, node_of, view_of};

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
    let full = view_of(page, mode).await?;
    let scoped = match &asked.scope {
        None => full.nodes.clone(),
        Some(scope) => {
            // A scope the page no longer has is a dead ref, not the whole
            // page: answering with everything would read as "here is your
            // subtree" and hide the one thing the caller has to re-read.
            let start =
                position_of(&full, node_of(scope)?).ok_or_else(|| super::stale_ref(scope))?;
            subtree(&full, start)
        }
    };
    let scoped = View { nodes: scoped };
    // A cursor the page no longer has — a re-render removed the node it named —
    // restarts at the top rather than silently skipping what is left.
    let from = match &asked.cursor {
        None => 0,
        Some(cursor) => match position_of(&scoped, node_of(cursor)?) {
            None => 0,
            Some(at) => at + 1,
        },
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
    let view = view_of(page, Mode::Interactive).await?;
    let matches: Vec<Value> = find::find(&view, &asked.query)
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

/// Where a ref sits in a view, or None when the node is not on it.
fn position_of(view: &View, backend_id: u64) -> Option<usize> {
    view.nodes
        .iter()
        .position(|node| node.backend_id == backend_id)
}

/// A node and everything under it. The view's depths are the nesting its lines
/// are written with, so a subtree ends at the first node no deeper than its
/// root.
fn subtree(view: &View, start: usize) -> Vec<ViewNode> {
    let Some(root) = view.nodes.get(start) else {
        return Vec::new();
    };
    let mut kept = vec![root.clone()];
    kept.extend(
        view.nodes
            .iter()
            .skip(start + 1)
            .take_while(|node| node.depth > root.depth)
            .cloned(),
    );
    kept
}
