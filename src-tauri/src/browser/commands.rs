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
pub mod batch;
pub mod input;
pub mod keys;
pub mod logs;
pub mod page_script;
pub mod read;
pub mod see;
pub mod shot;
pub mod tabs;
pub mod wait;

use serde::de::DeserializeOwned;
use serde_json::Value;

use devboule_protocol::{BrowserCaller, BrowserError, BrowserErrorCode, BrowserExecuteRequest};

use super::ax::AxTree;
use super::cdp::{Bounded, CdpError, Page, WebviewPage};
use super::deadline::Deadline;
use super::registry::{BrowserRegistry, TabInfo};
use super::tab_guard;
use super::view;
use super::view_walk;

/// The failure's wire name, for the one line this app writes per command. The
/// daemon's own log spells it the same way, so the two read alike.
pub fn code_name(code: BrowserErrorCode) -> &'static str {
    match code {
        BrowserErrorCode::NoHost => "browser_no_host",
        BrowserErrorCode::Timeout => "browser_timeout",
        BrowserErrorCode::Busy => "browser_busy",
        BrowserErrorCode::ResultTooLarge => "browser_result_too_large",
        BrowserErrorCode::ArgsTooLarge => "browser_args_too_large",
        BrowserErrorCode::HostError => "browser_host_error",
        BrowserErrorCode::UnsupportedCommand => "browser_unsupported_command",
        BrowserErrorCode::OwnerUnavailable => "browser_owner_unavailable",
        BrowserErrorCode::TabNotFound => "browser_tab_not_found",
    }
}

/// The commands this host runs, as the daemon is told at registration. The
/// list and the dispatch below are one thing: a name here that the dispatch
/// does not know is a command the daemon would route and this app would
/// refuse.
pub const COMMANDS: [&str; 20] = [
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
    "act",
    "screenshot",
    "click_at",
    "read_text",
    "console_logs",
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

/// One page's accessible tree, as the runtime computed it.
pub async fn tree_of(page: &dyn Page) -> Result<AxTree, BrowserError> {
    super::ax::tree(page).await.map_err(cdp_failure)
}

/// The view of one page, in the mode the caller asked for.
pub async fn view_of(page: &dyn Page, mode: view::Mode) -> Result<view::View, BrowserError> {
    Ok(view_walk::compact(&tree_of(page).await?, mode))
}

/// The commands that only read a page. They never wait for the tab and never
/// hold it: a `wait_for` that polled for twelve seconds under the tab's lock
/// would leave the pane's own present waiting just as long. They do put a
/// parked page at its pane's size, which is safe without the lock because the
/// override is checked against the live parked state on both sides of its call.
const READS: [&str; 6] = [
    "snapshot",
    "find",
    "wait_for",
    "screenshot",
    "read_text",
    "console_logs",
];

/// Run one command the daemon pushed to this host.
pub async fn dispatch(
    app: &tauri::AppHandle,
    registry: &BrowserRegistry,
    request: &BrowserExecuteRequest,
    deadline: Deadline,
) -> Result<Value, BrowserError> {
    let command = request.command.as_str();
    if matches!(command, "new_tab" | "list_tabs") {
        return tabs::run(
            app,
            registry,
            &request.caller,
            command,
            &request.args,
            deadline,
        )
        .await;
    }
    let browser_id = browser_id(&request.args)?;
    let tab = resolve(registry, &request.caller, &browser_id)?;
    if READS.contains(&command) {
        let page = WebviewPage::new(app, &tab.label);
        return on_tab(&tab, &page, command, &request.args, deadline).await;
    }
    let _held = tab_guard::hold(registry, &browser_id, deadline).await?;
    // What the tab was when this command queued is not what it is now: the
    // pane may have shown, parked or closed it while the lock was held.
    let tab = resolve(registry, &request.caller, &browser_id)?;
    if command == "close_tab" {
        return tabs::close_tab(app, registry, &tab);
    }
    let page = WebviewPage::new(app, &tab.label);
    on_tab(&tab, &page, command, &request.args, deadline).await
}

/// Run one command against one page. Everything below the tab commands goes
/// through here, so the whole command layer is reachable with a fake page, and
/// every call it makes is cut to the command's deadline.
pub async fn on_tab(
    tab: &TabInfo,
    page: &dyn Page,
    command: &str,
    args: &Value,
    deadline: Deadline,
) -> Result<Value, BrowserError> {
    let bounded = Bounded::new(page, deadline);
    let page: &dyn Page = &bounded;
    match command {
        "navigate" => tabs::navigate(tab, page, args, deadline).await,
        "act" => batch::run(tab, page, args, deadline).await,
        "snapshot" => see::snapshot(tab, page, args).await,
        "find" => see::find(tab, page, args).await,
        "screenshot" => shot::screenshot(tab, page, args).await,
        "read_text" => read::read_text(tab, page, args).await,
        // The one command that asks the page nothing: what the page said is
        // already in this process, and a console entry is only worth reading
        // after something has happened on the page.
        "console_logs" => logs::logs(&tab.browser_id, args),
        "click" | "fill" | "type" | "press" | "select" | "check" | "hover" | "scroll"
        | "click_at" => input::run(tab, page, command, args, deadline).await,
        "wait_for" => wait::wait_for(tab, page, args, deadline).await,
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

#[cfg(test)]
#[path = "commands_time_tests.rs"]
mod time_tests;
