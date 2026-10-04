//! Page script run on the page's own terms: the function, what it runs on, and
//! what it returned. Input goes through CDP's `Input.*`; this is for the three
//! things a person's input cannot do — emptying a field, choosing an option and
//! reading a document's readable text — and they go through the page's own DOM
//! so a framework's value tracker sees them.

use serde_json::{json, Value};

use devboule_protocol::BrowserError;

use super::super::cdp::Page;
use super::act::call;
use super::host_error;

/// Run one function on the page's own node, and hand back what it returned.
///
/// `Runtime.callFunctionOn` needs a live `objectId` — the one from
/// `DOM.resolveNode` — which is why the page's script is reached per call and
/// never held: an object id belongs to the node that produced it.
pub async fn on_node(
    page: &dyn Page,
    node: u64,
    function: &str,
    args: Value,
) -> Result<Value, BrowserError> {
    let object = object_of(page, node).await?;
    on_object(page, &object, function, args).await
}

/// The `objectId` a node is reached by, resolved fresh. A dead node refuses
/// here, and the runtime's one refusal for that is `stale_ref:`.
pub async fn object_of(page: &dyn Page, node: u64) -> Result<String, BrowserError> {
    let resolved = call(page, "DOM.resolveNode", json!({ "backendNodeId": node })).await?;
    resolved
        .get("object")
        .and_then(|object| object.get("objectId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| host_error("That node could not be resolved on the page."))
}

/// The same call on an object the page itself named — `document.body`, say,
/// which has no `backendDOMNodeId` to resolve.
pub async fn on_object(
    page: &dyn Page,
    object: &str,
    function: &str,
    args: Value,
) -> Result<Value, BrowserError> {
    run(
        page,
        "Runtime.callFunctionOn",
        json!({
            "objectId": object,
            "functionDeclaration": function,
            "arguments": args,
            "returnByValue": true,
        }),
    )
    .await
}

/// The same call where the page stands, with no object to run it on: the
/// expression invokes the function on whatever `this` names, which is how a
/// whole document is read without resolving a node for it first.
pub async fn evaluated(page: &dyn Page, expression: &str) -> Result<Value, BrowserError> {
    run(
        page,
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true,
            "silent": true,
        }),
    )
    .await
}

async fn run(page: &dyn Page, method: &str, params: Value) -> Result<Value, BrowserError> {
    let answered = call(page, method, params).await?;
    // A function that threw is still a successful protocol answer, with the
    // failure beside an absent value: read as `null`, it would let `fill` type
    // into a field the clear never emptied.
    if let Some(thrown) = answered.get("exceptionDetails") {
        return Err(host_error(format!(
            "The page's script failed on that node: {}",
            thrown_text(thrown)
        )));
    }
    Ok(answered
        .get("result")
        .and_then(|result| result.get("value"))
        .cloned()
        .unwrap_or(Value::Null))
}

/// What a thrown script said, short enough to read: the exception's own
/// description when the runtime sent one, else the `text` beside it.
fn thrown_text(thrown: &Value) -> String {
    let said = thrown
        .get("exception")
        .and_then(|exception| exception.get("description"))
        .or_else(|| thrown.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("it threw");
    said.chars().take(200).collect()
}

/// Empty a field the way a person would: through the native setter a
/// framework's value tracker wraps, then the events it listens for. A plain
/// `.value = ""` is invisible to that tracker, which is most of them.
pub const CLEAR: &str = r#"function () {
  if (this.isContentEditable) {
    this.textContent = "";
  } else if ("value" in this) {
    const setter = Object.getOwnPropertyDescriptor(
      Object.getPrototypeOf(this), "value"
    ).set;
    setter.call(this, "");
  } else {
    return false;
  }
  this.dispatchEvent(new Event("input", { bubbles: true }));
  this.dispatchEvent(new Event("change", { bubbles: true }));
  return true;
}"#;

/// Pick an option by value or by the label the page shows, and fire what a
/// person's choice fires.
pub const CHOOSE: &str = r#"function (wanted) {
  const options = this.options ? Array.from(this.options) : [];
  const wantedLabel = (wanted.label || "").trim();
  const hit = options.find((option) => option.value === wanted.value)
    || options.find((option) => (option.label || option.textContent || "").trim() === wantedLabel);
  if (!hit) return null;
  const setter = Object.getOwnPropertyDescriptor(
    Object.getPrototypeOf(this), "value"
  ).set;
  setter.call(this, hit.value);
  this.dispatchEvent(new Event("input", { bubbles: true }));
  this.dispatchEvent(new Event("change", { bubbles: true }));
  return hit.value;
}"#;
