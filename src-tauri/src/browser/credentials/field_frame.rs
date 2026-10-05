//! Which frame a field is in, and so which site it belongs to.
//!
//! The answer is read from the browser's own frame tree and DOM, never from
//! the page's scripts, which can override what they report about themselves.

use serde_json::{json, Value};

use devboule_protocol::BrowserError;

use super::super::cdp::Page;
use super::super::commands::{act, host_error};
use super::origin::{self, Origin};

/// Every frame (id, address) of a `Page.getFrameTree` answer, parents before
/// children, the page's own frame first.
pub(super) fn frames_of(tree: &Value) -> Vec<(String, String)> {
    fn walk(node: &Value, into: &mut Vec<(String, String)>) {
        if let Some(frame) = node.get("frame") {
            if let (Some(id), Some(url)) = (frame["id"].as_str(), frame["url"].as_str()) {
                into.push((id.to_owned(), url.to_owned()));
            }
        }
        if let Some(children) = node.get("childFrames").and_then(Value::as_array) {
            for child in children {
                walk(child, into);
            }
        }
    }
    let mut every = Vec::new();
    walk(tree.get("frameTree").unwrap_or(&Value::Null), &mut every);
    every
}

/// The canonical origin of one frame. A frame the tree does not carry, and an
/// address this app cannot compare as an origin — `about:blank` and the other
/// opaque ones — are refused here rather than guessed at.
pub(super) fn origin_in(every: &[(String, String)], frame: &str) -> Result<Origin, BrowserError> {
    let (_, url) = every
        .iter()
        .find(|(id, _)| id == frame)
        .ok_or_else(|| host_error("That field's frame is not in this page's frame tree."))?;
    origin::of_page(url).map_err(|why| {
        host_error(format!(
            "That field's own address cannot be compared with a saved login: {why}"
        ))
    })
}

/// Whether a `DOM.describeNode` subtree holds the node, looking through shadow
/// roots and into the documents of the frames it owns.
fn holds(subtree: &Value, node: u64) -> bool {
    if subtree["backendNodeId"].as_u64() == Some(node) {
        return true;
    }
    ["children", "shadowRoots"]
        .iter()
        .filter_map(|key| subtree.get(*key).and_then(Value::as_array))
        .flatten()
        .chain(subtree.get("contentDocument"))
        .any(|inner| holds(inner, node))
}

/// The frame a node is in. `DOM.describeNode` names a frame only for the
/// element that owns it, so the answer is the deepest frame whose owner holds
/// the node, else the page's own frame. A frame whose owner cannot be named is
/// a refusal, not a guess: the field might be inside it.
pub(super) async fn frame_of(
    page: &dyn Page,
    node: u64,
    every: &[(String, String)],
) -> Result<String, BrowserError> {
    act::call(
        page,
        "DOM.describeNode",
        json!({ "backendNodeId": node, "depth": 0 }),
    )
    .await?;
    let mut inside = every
        .first()
        .map(|(id, _)| id.clone())
        .ok_or_else(|| host_error("This page reports no frame of its own."))?;
    for (frame, _) in every.iter().skip(1) {
        let owner = act::call(page, "DOM.getFrameOwner", json!({ "frameId": frame })).await?;
        let Some(owner) = owner["backendNodeId"].as_u64() else {
            return Err(host_error(
                "That field is not in a frame this app can name.",
            ));
        };
        let owned = act::call(
            page,
            "DOM.describeNode",
            json!({ "backendNodeId": owner, "depth": -1, "pierce": true }),
        )
        .await?;
        if holds(&owned["node"], node) {
            inside = frame.clone();
        }
    }
    Ok(inside)
}

#[cfg(test)]
#[path = "field_frame_tests.rs"]
mod tests;
