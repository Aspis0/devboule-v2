//! The browser commands an agent may run, and the one place they are named.
//!
//! Every command answers with one JSON object, and every failure answers with
//! the protocol's own code: a ref whose node is gone is `stale_ref:`, a tab of
//! another workspace is `browser_tab_not_found`, and anything else is a host
//! error carrying the runtime's text. There is no third answer.
//!
//! The tab commands (`new_tab`, `list_tabs`, `close_tab`) need the registry and
//! the app, because they are about tabs rather than about a page. Everything
//! else runs on one page, through [`cdp::Page`], which is what lets the whole
//! command layer be driven by canned answers in a test.

pub mod act;
pub mod input;
pub mod see;
pub mod tabs;
pub mod wait;

use serde::de::DeserializeOwned;
use serde_json::Value;

use devboule_protocol::{BrowserCaller, BrowserError, BrowserErrorCode, BrowserExecuteRequest};

use super::cdp::{CdpError, Page, WebviewPage};
use super::registry::{BrowserRegistry, TabInfo};
use super::view;

/// The commands this host runs, as the daemon is told at registration. The
/// list and the dispatch below are one thing: a name here that the dispatch
/// does not know is a command the daemon would route and this app would
/// refuse.
pub const COMMANDS: [&str; 15] = [
    "new_tab",
    "list_tabs",
    "close_tab",
    "navigate",
    "snapshot",
    "find",
    "click",
    "fill",
    "type",
    "press",
    "select",
    "check",
    "hover",
    "scroll",
    "wait_for",
];

/// A failure, in the protocol's own vocabulary.
pub fn host_error(message: impl Into<String>) -> BrowserError {
    BrowserError::daemon(BrowserErrorCode::HostError, message)
}

/// The same, with `retryable` false whatever the code says: a host failure is
/// a refusal by this app, and asking again will be refused the same way.
pub fn refused(code: BrowserErrorCode, message: impl Into<String>) -> BrowserError {
    BrowserError {
        code,
        message: message.into(),
        retryable: false,
    }
}

/// The tab a command is about, or the refusal it answers with.
///
/// Scope and existence are one answer on purpose: a tab of another workspace is
/// reported exactly as an unknown id is, because telling the two apart would
/// tell an agent in one workspace which ids exist in another.
pub fn resolve(
    registry: &BrowserRegistry,
    caller: &BrowserCaller,
    browser_id: &str,
) -> Result<TabInfo, BrowserError> {
    let tab = registry
        .tab_of(browser_id)
        .filter(|tab| Some(tab.workspace.as_str()) == caller.workspace_id.as_deref())
        .ok_or_else(|| tab_not_found(browser_id))?;
    Ok(tab)
}

pub fn tab_not_found(browser_id: &str) -> BrowserError {
    refused(
        BrowserErrorCode::TabNotFound,
        format!("No browser tab {browser_id} in this workspace."),
    )
}

/// The `browserId` an agent addressed, read from the arguments.
pub fn browser_id(args: &Value) -> Result<String, BrowserError> {
    args.get("browserId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| host_error("This command needs a browserId."))
}

/// One command's arguments, as the command declares them. A missing or
/// mistyped field is a refusal naming the field, never a default.
pub fn args_of<T: DeserializeOwned>(args: &Value) -> Result<T, BrowserError> {
    serde_json::from_value(args.clone()).map_err(|error| host_error(error.to_string()))
}

/// Turn a protocol failure into this app's answer. The one rule: a dead ref
/// says so in the message, because the caller's only move is to take a new
/// snapshot and every other message would send it looking for the fault.
pub fn cdp_failure(error: CdpError) -> BrowserError {
    host_error(error.message())
}

/// A ref, as the node id it names.
pub fn node_of(reference: &str) -> Result<u64, BrowserError> {
    view::parse_ref(reference).ok_or_else(|| stale_ref(reference))
}

/// The one refusal a dead ref ever gets, worded once so every command says it
/// the same way: the caller's only move is a new snapshot, and a different
/// sentence would send it looking for the fault somewhere else.
pub fn stale_ref(reference: &str) -> BrowserError {
    host_error(format!(
        "stale_ref: {reference} is not a node of this page any more; take a new snapshot"
    ))
}

/// The view of one page, in the mode the caller asked for.
pub async fn view_of(page: &dyn Page, mode: view::Mode) -> Result<view::View, BrowserError> {
    let tree = super::ax::tree(page).await.map_err(cdp_failure)?;
    Ok(view::compact(&tree, mode))
}

/// Run one command the daemon pushed to this host.
pub async fn dispatch(
    app: &tauri::AppHandle,
    registry: &BrowserRegistry,
    request: &BrowserExecuteRequest,
) -> Result<Value, BrowserError> {
    match request.command.as_str() {
        "new_tab" | "list_tabs" | "close_tab" => {
            tabs::run(
                app,
                registry,
                &request.caller,
                &request.command,
                &request.args,
            )
            .await
        }
        _ => {
            let browser_id = browser_id(&request.args)?;
            let tab = resolve(registry, &request.caller, &browser_id)?;
            let page = WebviewPage::new(app, &tab.label);
            on_tab(&tab, &page, &request.command, &request.args).await
        }
    }
}

/// Run one command against one page. Everything below the tab commands goes
/// through here, so the whole command layer is reachable with a fake page.
pub async fn on_tab(
    tab: &TabInfo,
    page: &dyn Page,
    command: &str,
    args: &Value,
) -> Result<Value, BrowserError> {
    match command {
        "navigate" => tabs::navigate(tab, page, args).await,
        "snapshot" => see::snapshot(tab, page, args).await,
        "find" => see::find(tab, page, args).await,
        "click" | "fill" | "type" | "press" | "select" | "check" | "hover" | "scroll" => {
            input::run(tab, page, command, args).await
        }
        "wait_for" => wait::wait_for(tab, page, args).await,
        // The daemon only routes a command this host registered, so this is a
        // wiring fault rather than a caller's mistake — and it is reported as
        // such rather than as an empty answer.
        other => Err(host_error(format!(
            "{other} is not a command this app runs."
        ))),
    }
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
