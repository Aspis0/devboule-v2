//! Whether a node is a field a saved login's value belongs in, and whether it
//! is the one holding the focus when the value is about to go in.
//!
//! The kind of a field is read from the browser's own DOM (`DOM.describeNode`,
//! `DOM.getBoxModel`), not from the page's scripts. Focus is the page's own
//! statement about itself: it narrows the gap between the last check and the
//! insert, and cannot close it.

use serde_json::{json, Value};

use devboule_protocol::BrowserError;

use super::super::cdp::Page;
use super::super::commands::page_script;
use super::super::commands::{act, host_error};

/// What a call asked a field to hold.
#[derive(Clone, Copy)]
pub(super) enum Kind {
    Username,
    Password,
}

/// True when this node is its root's focused element and that root's document
/// has the focus, which is where `Input.insertText` lands.
const FOCUSED: &str = r#"function () {
  return this.getRootNode().activeElement === this && this.ownerDocument.hasFocus();
}"#;

/// The attribute `name` of a `DOM.describeNode` node, which the protocol lists
/// as one flat run of names and values. A present attribute with no value is `""`.
fn attribute<'a>(node: &'a Value, name: &str) -> Option<&'a str> {
    node["attributes"]
        .as_array()?
        .chunks(2)
        .find(|pair| {
            pair[0]
                .as_str()
                .is_some_and(|key| key.eq_ignore_ascii_case(name))
        })
        .map(|pair| pair.get(1).and_then(Value::as_str).unwrap_or_default())
}

/// Refuse a node that is not an enabled, editable, shown input of the kind the
/// call named for it. A password goes to a `type=password` input only, and a
/// username to a text-like one: neither is ever sent to whatever a ref points at.
pub(super) async fn fillable(page: &dyn Page, node: u64, kind: Kind) -> Result<(), BrowserError> {
    let described = act::call(
        page,
        "DOM.describeNode",
        json!({ "backendNodeId": node, "depth": 0 }),
    )
    .await?;
    let element = &described["node"];
    if !element["nodeName"]
        .as_str()
        .is_some_and(|name| name.eq_ignore_ascii_case("input"))
    {
        return Err(refusal(kind, "is not an input"));
    }
    if attribute(element, "disabled").is_some() {
        return Err(refusal(kind, "is disabled"));
    }
    if attribute(element, "readonly").is_some() {
        return Err(refusal(kind, "is read-only"));
    }
    let given = attribute(element, "type")
        .unwrap_or_default()
        .to_lowercase();
    let right_kind = match kind {
        Kind::Password => given == "password",
        Kind::Username => matches!(given.as_str(), "" | "text" | "email" | "tel"),
    };
    if !right_kind {
        let what = if given.is_empty() { "text" } else { &given };
        return Err(refusal(kind, &format!("is a `{what}` input")));
    }
    let model = act::call(page, "DOM.getBoxModel", json!({ "backendNodeId": node }))
        .await
        .map_err(|_| refusal(kind, "is not shown on the page"))?;
    let shown = |side: &str| model["model"][side].as_f64().is_some_and(|size| size > 0.0);
    if !(shown("width") && shown("height")) {
        return Err(refusal(kind, "is not shown on the page"));
    }
    Ok(())
}

fn refusal(kind: Kind, why: &str) -> BrowserError {
    let (arg, wanted) = match kind {
        Kind::Password => ("passwordRef", "a password input"),
        Kind::Username => ("usernameRef", "a text, email or tel input"),
    };
    host_error(format!(
        "{arg} must name {wanted} that can be typed into, and that field {why}; take a new \
         snapshot and pick the field again."
    ))
}

/// Refuse unless the node is the focused element of a focused document.
pub(super) async fn focused(page: &dyn Page, node: u64) -> Result<(), BrowserError> {
    let holding = page_script::on_node(page, node, FOCUSED, json!([])).await?;
    if holding != Value::Bool(true) {
        return Err(host_error(
            "That field does not hold the focus, so nothing was typed; take a new snapshot and \
             look again.",
        ));
    }
    Ok(())
}
