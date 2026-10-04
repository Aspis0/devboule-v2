//! `read_text`: what the page says, in the order a person would read it.
//!
//! A snapshot deliberately drops everything that is not a control, which is
//! why an agent cannot answer "what does this page say about X" from one: the
//! prose is not in it. This command is the other half of the view — the same
//! page as text, with a heading as a heading and a link as its words and where
//! they go.
//!
//! **The text is read by one bounded page-side function, not built in Rust from
//! the accessible tree.** The tree has no href in it at all — a link is a role
//! and a name, never an address — so `[text](href)` would need a
//! `DOM.describeNode` per link, on a page of four hundred of them, to learn
//! what the page's own DOM already holds. The function returns plain text: no
//! markup crosses back into this app, so nothing a page writes can become
//! anything but characters in a string.
//!
//! What it skips: scripts, styles, and anything hidden — `display: none`,
//! `visibility: hidden`, `hidden`, `aria-hidden` — which is what a person
//! cannot read either. A frame or a canvas is not this document's text and is
//! left out rather than read empty.

use serde::Deserialize;
use serde_json::{json, Value};

use devboule_protocol::BrowserError;

use super::super::cdp::Page;
use super::super::registry::TabInfo;
use super::super::view::VIEW_BUDGET;
use super::{act, args_of, host_error, node_of, page_script};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadArgs {
    scope: Option<String>,
    cursor: Option<String>,
}

pub async fn read_text(
    tab: &TabInfo,
    page: &dyn Page,
    args: &Value,
) -> Result<Value, BrowserError> {
    let asked: ReadArgs = args_of(args)?;
    // A parked page reads as the layout it would have on screen: a document
    // that collapses its text into a menu at two pixels says nothing here.
    act::ready(tab, page).await?;
    let from = match &asked.cursor {
        None => 0,
        Some(cursor) => cursor
            .parse::<usize>()
            .map_err(|_| host_error(format!("{cursor} is not a position in this text.")))?,
    };
    let whole = extract(page, asked.scope.as_deref(), from + VIEW_BUDGET).await?;
    let rest: String = whole.chars().skip(from).collect();
    let text: String = rest.chars().take(VIEW_BUDGET).collect();
    let truncated = rest.chars().count() > text.chars().count();
    Ok(json!({
        "url": tab.url,
        "title": tab.title,
        "text": text,
        "truncated": truncated,
        // Where the next call starts. An offset and not a ref, because text has
        // no nodes to name: a page that moved re-reads from this character.
        "cursor": truncated.then(|| (from + VIEW_BUDGET).to_string()),
    }))
}

/// The page's readable text, asked for from the page itself.
///
/// `budget` is what is left to read rather than what one answer carries, so a
/// continuation reads past the text the first call already sent.
async fn extract(
    page: &dyn Page,
    scope: Option<&str>,
    budget: usize,
) -> Result<String, BrowserError> {
    let whole = match scope {
        Some(reference) => {
            let object = page_script::object_of(page, node_of(reference)?).await?;
            page_script::on_object(page, &object, READABLE, json!([budget])).await?
        }
        None => {
            page_script::evaluated(page, &format!("({READABLE}).call(document.body, {budget})"))
                .await?
        }
    };
    Ok(whole.as_str().unwrap_or_default().to_owned())
}

/// The function that reads a node's subtree as prose, called on `this`.
///
/// Written to be cheap on a page that is mostly one big list: it stops at its
/// node budget and at `budget` characters, and it asks for a computed style
/// only once per element, through `shown` and the block check below.
pub const READABLE: &str = r##"function (budget) {
  const SKIP = ["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "HEAD", "LINK",
                "META", "SVG", "CANVAS", "IMG", "IFRAME", "OBJECT", "VIDEO", "AUDIO"];
  const NODES = 20000, LINES = 4000;
  const lines = [];
  let used = 0, seen = 0, buffer = "";
  const flat = (text) => (text || "").replace(/\s+/g, " ");
  const style = (el) => window.getComputedStyle(el);
  const shown = (el) =>
    !el.hidden &&
    el.getAttribute("aria-hidden") !== "true" &&
    style(el).display !== "none" &&
    style(el).visibility !== "hidden";
  const add = (line) => {
    line = flat(line).trim();
    if (!line || lines.length >= LINES) return;
    const left = budget - used;
    if (left <= 0) return;
    // A line longer than what is left is cut at a word, so the page-side bound
    // is a bound: without this one paragraph would carry the whole budget past
    // it and the walk would go on.
    if (line.length > left) line = line.slice(0, left).replace(/\s+\S*$/, "");
    if (!line) return;
    lines.push(line);
    used += line.length + 1;
  };
  // A link's own label, which is its subtree read as one line.
  //
  // Iterative, and on the page's own node budget. A link can nest markup as
  // deep as a page likes: a reader that recursed would overflow the renderer's
  // stack, and one that walked the subtree for free would read nodes the rest
  // of the walk had budgeted away.
  const inside = (node) => {
    const stack = [node];
    let text = "";
    while (stack.length) {
      const current = stack.pop();
      if (current.nodeType === 3) { text += flat(current.nodeValue).trim() + " "; continue; }
      if (current.nodeType !== 1 || SKIP.indexOf(current.tagName) >= 0) continue;
      if (++seen > NODES || !shown(current)) break;
      // Pushed back to front, so the walk stays in document order.
      for (let child = current.lastChild; child; child = child.previousSibling) stack.push(child);
    }
    return flat(text).trim();
  };
  const walk = (node) => {
    if (used >= budget || ++seen > NODES) return;
    if (node.nodeType === 3) { buffer += flat(node.nodeValue) + " "; return; }
    if (node.nodeType !== 1) return;
    const tag = node.tagName;
    if (SKIP.indexOf(tag) >= 0 || !shown(node)) return;
    if (tag === "A") {
      const label = inside(node);
      const href = flat(node.getAttribute("href"));
      const text = label && href ? "[" + label + "](" + href + ")" : label || href;
      // A link goes into the line its parent is building: "click [here](…)"
      // is one sentence, and dropping the words around it loses them.
      if (text) buffer = flat(buffer + " " + text);
      return;
    }
    const heading = /^H([1-6])$/.exec(tag);
    if (heading || tag === "LI") { add(buffer); buffer = ""; }
    for (let child = node.firstChild; child; child = child.nextSibling) walk(child);
    buffer = flat(buffer);
    if (heading) { add("#".repeat(Number(heading[1])) + " " + buffer); buffer = ""; }
    else if (tag === "LI") { add("- " + buffer); buffer = ""; }
    else if (style(node).display === "block") { add(buffer); buffer = ""; }
    else if (buffer) { buffer += " "; }
  };
  walk(this);
  add(buffer);
  return lines.join("\n");
}"##;

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
