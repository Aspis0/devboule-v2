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

use std::sync::Arc;

use super::ax::AxTree;
use super::cdp::{Bounded, CdpError, Page, WebviewPage};
use super::credentials::fill_login;
use super::deadline::Deadline;
use super::destination::{Audience, DestinationPolicy};
use super::registry::{BrowserRegistry, TabInfo};
use super::scrub;
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
///
/// The two `fill_login` commands are here and not in the served table beside
/// them because they are not tools: `fill_login_preview` is what the daemon
/// asks before it puts the choice to a person, and it has no agent-facing
/// name at all.
pub const COMMANDS: [&str; 22] = [
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
    "fill_login_preview",
    "fill_login",
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
    match error {
        // A budget that ran out is the daemon's own deadline, not something the
        // page refused, and a caller that retries a timeout wants to know it
        // was a timeout.
        CdpError::OutOfTime => BrowserError {
            code: BrowserErrorCode::Timeout,
            message: "The browser command ran out of time before the page answered.".to_owned(),
            retryable: true,
        },
        other => host_error(other.message()),
    }
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

/// One page's accessible tree, with every password field's value replaced.
///
/// Every answer built from the accessible tree comes through here — the view,
/// a snapshot, a delta, a `find`, a `wait_for` — which is what makes the
/// replacement total for those. `read_text` is not one of them: it never reads
/// the tree, and its own page-side walk refuses the same two marks in
/// `commands::read` with its own tests.
pub async fn tree_of(page: &dyn Page) -> Result<AxTree, BrowserError> {
    let mut tree = super::ax::tree(page).await.map_err(cdp_failure)?;
    super::mask::redact(page, &mut tree)
        .await
        .map_err(cdp_failure)?;
    Ok(tree)
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

/// Which of the dispatch's two page arms runs `command`.
///
/// The saved-login pair is dispatched beside this table rather than through
/// [`on_tab`], because it builds this machine's vault out of the app handle.
/// The arm is named here so that a name registered and not run cannot reach
/// the dispatch as a refusal, which is what the registered-list test reads.
#[derive(PartialEq, Eq, Debug)]
enum Routed {
    /// `on_tab`, on a page the tab lock has been taken for.
    Tab,
    /// A saved login, over the vault this machine owns.
    SavedLogin,
}

fn routed(command: &str) -> Routed {
    if fill_login::COMMANDS.contains(&command) {
        Routed::SavedLogin
    } else {
        Routed::Tab
    }
}

/// The URL an agent command is about to open, when it names one. `navigate`
/// may instead carry a history action, which opens nothing new.
fn agent_named_url<'a>(command: &str, args: &'a Value) -> Option<&'a str> {
    match command {
        "navigate" | "new_tab" => args.get("url").and_then(Value::as_str),
        _ => None,
    }
}

/// The destination policy this app runs with. A process that never managed
/// one — a test app, not the product — gets the default: no exceptions.
fn policy_of(app: &tauri::AppHandle) -> Arc<DestinationPolicy> {
    use tauri::Manager;
    app.try_state::<Arc<DestinationPolicy>>()
        .map(|state| Arc::clone(state.inner()))
        .unwrap_or_else(|| Arc::new(DestinationPolicy::load(None)))
}

/// Run one command the daemon pushed to this host.
pub async fn dispatch(
    app: &tauri::AppHandle,
    registry: &BrowserRegistry,
    request: &BrowserExecuteRequest,
    deadline: Deadline,
) -> Result<Value, BrowserError> {
    let command = request.command.as_str();
    // The agent's own entry: a URL it names is checked before anything is
    // asked of a page or a child, and the refusal names the address.
    if let Some(raw) = agent_named_url(command, &request.args) {
        let target =
            super::url::accept(raw).map_err(|error| refused(BrowserErrorCode::HostError, error))?;
        if let Err(blocked) = policy_of(app).admit(&target, Audience::Agent) {
            return Err(host_error(blocked.message().to_string()));
        }
    }
    if matches!(command, "new_tab" | "list_tabs") {
        return scrubbed(
            tabs::run(
                app,
                registry,
                &request.caller,
                command,
                &request.args,
                deadline,
            )
            .await,
        );
    }
    let browser_id = browser_id(&request.args)?;
    let tab = resolve(registry, &request.caller, &browser_id)?;
    // The page owns whatever was typed into it, so what this process typed on
    // a site the tab has since left is dropped here, before the answer is
    // built from whatever is on screen now.
    scrub::left(&tab.browser_id, fill_login::origin_of(&tab).as_deref());
    if READS.contains(&command) {
        let page = WebviewPage::new(app, &tab.label);
        return scrubbed(on_tab(&tab, &page, command, &request.args, deadline).await);
    }
    let _held = tab_guard::hold(registry, &browser_id, deadline).await?;
    // What the tab was when this command queued is not what it is now: the
    // pane may have shown, parked or closed it while the lock was held.
    let tab = resolve(registry, &request.caller, &browser_id)?;
    if command == "close_tab" {
        return scrubbed(tabs::close_tab(app, registry, &tab));
    }
    // Every navigation this command causes is agent-driven until it answers,
    // whatever the pane is showing; the page's own hook reads this hold.
    let drive = registry.drive_of(&browser_id);
    let _driving = drive.as_ref().map(|drive| drive.begin());
    let page = WebviewPage::new(app, &tab.label);
    let outcome = match routed(command) {
        // The one command beside `on_tab`, so the bound `on_tab` puts on a page
        // has to be put here too: a saved login asks the page the same questions
        // and may wait no longer for the answers.
        Routed::SavedLogin => {
            let bounded = Bounded::new(&page, deadline);
            fill_login::run(app, &tab, &bounded, command, &request.args).await
        }
        Routed::Tab => on_tab(&tab, &page, command, &request.args, deadline).await,
    };
    let answer = scrubbed(outcome)?;
    // A navigation the hook refused while this command ran is reported to the
    // agent here, on the next answer it gets.
    let refused_navigation = drive.as_ref().and_then(|drive| drive.take_refusal());
    Ok(match refused_navigation {
        None => answer,
        Some(reason) => {
            let mut answer = answer;
            if let Some(object) = answer.as_object_mut() {
                object.insert("refusedNavigation".to_string(), Value::String(reason));
            }
            answer
        }
    })
}

/// The answer of one command, or the refusal of one, with whatever this process
/// typed into any tab taken out of it. The one place a browser answer leaves
/// this app.
fn scrubbed(outcome: Result<Value, BrowserError>) -> Result<Value, BrowserError> {
    match outcome {
        Ok(mut answer) => {
            scrub::clean(&mut answer);
            Ok(answer)
        }
        Err(mut error) => {
            scrub::clean_text(&mut error.message);
            Err(error)
        }
    }
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
