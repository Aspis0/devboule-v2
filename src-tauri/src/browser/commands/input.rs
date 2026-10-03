//! The commands that put input into a page: click, hover, fill, type, press,
//! select, check, scroll.
//!
//! Input goes through CDP's `Input.*` and not through page script, because a
//! page that accepts `element.click()` accepts a click it never saw — which is
//! how a toggle "succeeds" without changing, and why a rich editor that
//! swallows synthetic keystrokes accepts these.
//!
//! A ref is a `backendDOMNodeId`: it survives a re-render of the node and dies
//! with the navigation that replaced it. Every node call goes through
//! [`node_of`], so a dead ref answers `stale_ref:` from one place and is never
//! remapped to something else.

use serde::Deserialize;
use serde_json::{json, Value};

use devboule_protocol::BrowserError;

use super::super::cdp::Page;
use super::super::registry::TabInfo;
use super::act;
use super::{args_of, host_error, node_of};

#[derive(Deserialize)]
struct RefArgs {
    #[serde(rename = "ref")]
    reference: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClickArgs {
    #[serde(rename = "ref")]
    reference: String,
    button: Option<String>,
    click_count: Option<u32>,
    modifiers: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FillArgs {
    #[serde(rename = "ref")]
    reference: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TypeArgs {
    text: String,
    #[serde(rename = "ref")]
    reference: Option<String>,
}

#[derive(Deserialize)]
struct PressArgs {
    key: String,
    #[serde(rename = "ref")]
    reference: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectArgs {
    #[serde(rename = "ref")]
    reference: String,
    value: Option<String>,
    label: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckArgs {
    #[serde(rename = "ref")]
    reference: String,
    checked: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScrollArgs {
    #[serde(rename = "ref")]
    reference: Option<String>,
    direction: Option<String>,
    amount: Option<f64>,
}

pub async fn run(
    tab: &TabInfo,
    page: &dyn Page,
    command: &str,
    args: &Value,
) -> Result<Value, BrowserError> {
    match command {
        "click" => click(tab, page, args).await,
        "fill" => fill(tab, page, args).await,
        "type" => type_into(tab, page, args).await,
        "press" => press(tab, page, args).await,
        "select" => select(tab, page, args).await,
        "check" => check(tab, page, args).await,
        "hover" => hover(tab, page, args).await,
        _ => scroll(tab, page, args).await,
    }
}

pub async fn click(tab: &TabInfo, page: &dyn Page, args: &Value) -> Result<Value, BrowserError> {
    let asked: ClickArgs = args_of(args)?;
    let node = node_of(&asked.reference)?;
    let start = act::read(tab, page).await?;
    act::ready(tab, page).await?;
    act::into_view(page, node).await?;
    let at = act::centre(page, node).await?;
    let button = asked.button.as_deref().unwrap_or("left");
    let count = asked.click_count.unwrap_or(1);
    let modifiers = asked.modifiers.unwrap_or(0);
    act::mouse(page, "mousePressed", at, button, count, modifiers).await?;
    act::mouse(page, "mouseReleased", at, button, count, modifiers).await?;
    act::answer(tab, page, start, Some(node)).await
}

pub async fn hover(tab: &TabInfo, page: &dyn Page, args: &Value) -> Result<Value, BrowserError> {
    let asked: RefArgs = args_of(args)?;
    let node = node_of(&asked.reference)?;
    let start = act::read(tab, page).await?;
    act::ready(tab, page).await?;
    act::into_view(page, node).await?;
    act::mouse(
        page,
        "mouseMoved",
        act::centre(page, node).await?,
        "none",
        0,
        0,
    )
    .await?;
    act::answer(tab, page, start, Some(node)).await
}

/// Replace a field's contents: the clear is page script, the text is not.
/// A script that assigns `.value` on its own leaves a value tracker believing
/// nothing changed, while inserted text arrives as input a person made.
pub async fn fill(tab: &TabInfo, page: &dyn Page, args: &Value) -> Result<Value, BrowserError> {
    let asked: FillArgs = args_of(args)?;
    let node = node_of(&asked.reference)?;
    let start = act::read(tab, page).await?;
    act::ready(tab, page).await?;
    act::into_view(page, node).await?;
    act::call(page, "DOM.focus", json!({ "backendNodeId": node })).await?;
    act::on_node(page, node, act::CLEAR, json!([])).await?;
    act::call(page, "Input.insertText", json!({ "text": asked.text })).await?;
    act::answer(tab, page, start, Some(node)).await
}

pub async fn type_into(
    tab: &TabInfo,
    page: &dyn Page,
    args: &Value,
) -> Result<Value, BrowserError> {
    let asked: TypeArgs = args_of(args)?;
    let start = act::read(tab, page).await?;
    act::ready(tab, page).await?;
    let node = focus(page, &asked.reference).await?;
    act::call(page, "Input.insertText", json!({ "text": asked.text })).await?;
    act::answer(tab, page, start, node).await
}

pub async fn press(tab: &TabInfo, page: &dyn Page, args: &Value) -> Result<Value, BrowserError> {
    let asked: PressArgs = args_of(args)?;
    let start = act::read(tab, page).await?;
    act::ready(tab, page).await?;
    let node = focus(page, &asked.reference).await?;
    act::press_key(page, &asked.key).await?;
    act::answer(tab, page, start, node).await
}

pub async fn select(tab: &TabInfo, page: &dyn Page, args: &Value) -> Result<Value, BrowserError> {
    let asked: SelectArgs = args_of(args)?;
    if asked.value.is_none() && asked.label.is_none() {
        return Err(host_error("select needs a value or a label to choose."));
    }
    let node = node_of(&asked.reference)?;
    let start = act::read(tab, page).await?;
    act::ready(tab, page).await?;
    act::into_view(page, node).await?;
    let chosen = act::on_node(
        page,
        node,
        act::CHOOSE,
        json!([{ "value": asked.value, "label": asked.label }]),
    )
    .await?;
    if chosen.is_null() {
        return Err(host_error("This control has no such option."));
    }
    act::answer(tab, page, start, Some(node)).await
}

/// Put a control in the state the caller asked for, and touch it only when it
/// is not already there: clicking a checked box unchecks it.
pub async fn check(tab: &TabInfo, page: &dyn Page, args: &Value) -> Result<Value, BrowserError> {
    let asked: CheckArgs = args_of(args)?;
    let node = node_of(&asked.reference)?;
    let start = act::read(tab, page).await?;
    act::ready(tab, page).await?;
    let now = act::node_of_state(&start.view, node).and_then(|node| node.checked);
    if now != Some(asked.checked) {
        act::into_view(page, node).await?;
        let at = act::centre(page, node).await?;
        act::mouse(page, "mousePressed", at, "left", 1, 0).await?;
        act::mouse(page, "mouseReleased", at, "left", 1, 0).await?;
    }
    act::answer(tab, page, start, Some(node)).await
}

pub async fn scroll(tab: &TabInfo, page: &dyn Page, args: &Value) -> Result<Value, BrowserError> {
    let asked: ScrollArgs = args_of(args)?;
    let start = act::read(tab, page).await?;
    act::ready(tab, page).await?;
    let node = match &asked.reference {
        Some(reference) => {
            let node = node_of(reference)?;
            act::into_view(page, node).await?;
            Some(node)
        }
        None => None,
    };
    if let Some(direction) = &asked.direction {
        let notches = asked.amount.unwrap_or(3.0);
        let delta = match direction.as_str() {
            "up" => -notches * act::NOTCH,
            "down" => notches * act::NOTCH,
            other => return Err(host_error(format!("{other} is not a scroll direction."))),
        };
        // The wheel goes to the middle of the page's own viewport, which is
        // the metrics `ready` just put there.
        let middle = (tab.size.width / 2.0, tab.size.height / 2.0);
        act::wheel(page, middle, delta).await?;
    }
    act::answer(tab, page, start, node).await
}

/// Focus a node the caller named, or nothing when it named none: `type` and
/// `press` type where the page already has the caret.
async fn focus(page: &dyn Page, reference: &Option<String>) -> Result<Option<u64>, BrowserError> {
    let Some(reference) = reference else {
        return Ok(None);
    };
    let node = node_of(reference)?;
    act::into_view(page, node).await?;
    act::call(page, "DOM.focus", json!({ "backendNodeId": node })).await?;
    Ok(Some(node))
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
