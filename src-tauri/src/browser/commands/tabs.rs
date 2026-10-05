//! The commands that are about a tab rather than about a node: `new_tab`,
//! `list_tabs`, `close_tab`, and `navigate` (which changes where the tab's
//! page is, not what is on it).
//!
//! A tab an agent opens is a real page, parked, in the caller's workspace,
//! and it shows up in that workspace's strip as a chip. It is the user's to
//! open and close from then on, which is why each of these reports the change
//! as an event and why `close_tab` by an agent takes the chip with it.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager};

use devboule_protocol::{BrowserCaller, BrowserError, BrowserErrorCode};

use super::super::cdp::Page;
use super::super::deadline::Deadline;
use super::super::registry::{BrowserRegistry, TabInfo};
use super::super::scrub;
use super::super::tab;
use super::{act, host_error, refused, tab_not_found};

/// The app-wide event the strip listens for. One name with a tag, because a
/// chip appearing and a chip going away are the same fact about the same list.
pub const TAB_EVENT: &str = "browser:tab";

/// What the frontend is told about a tab it did not open itself.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TabEvent {
    Opened {
        browser_id: String,
        workspace_id: String,
        url: String,
    },
    Closed {
        browser_id: String,
    },
    /// What a page now says, for a tab no pane is showing. The strip persists
    /// it, so an agent-opened chip is named by the page's own title from the
    /// moment the page has one.
    State {
        browser_id: String,
        url: String,
        title: Option<String>,
        favicon: Option<String>,
    },
}

#[derive(Deserialize)]
struct NewTabArgs {
    url: String,
}

/// Visible because `act` checks a step's own shape against it before the
/// batch runs (`batch::checked`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NavigateArgs {
    url: Option<String>,
    action: Option<String>,
}

pub async fn run(
    app: &AppHandle,
    registry: &BrowserRegistry,
    caller: &BrowserCaller,
    command: &str,
    args: &Value,
    deadline: Deadline,
) -> Result<Value, BrowserError> {
    match command {
        "new_tab" => new_tab(app, registry, caller, args, deadline).await,
        "list_tabs" => list_tabs(registry, caller),
        other => Err(host_error(format!(
            "{other} is not a command this app runs."
        ))),
    }
}

/// A fresh tab id. A v4 UUID built from the OS random source and never from a
/// clock: two tabs opened in the same millisecond must not collide, and the id
/// is a webview label once it is minted.
fn mint_id() -> Result<String, BrowserError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| host_error(error.to_string()))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// The workspace an agent's tab belongs to. Without one the tab could not be
/// scoped, listed or shown, so there is nothing to create.
fn workspace_of(caller: &BrowserCaller) -> Result<String, BrowserError> {
    caller
        .workspace_id
        .clone()
        .filter(|workspace| !workspace.is_empty())
        .ok_or_else(|| host_error("This browser command has no workspace to open a tab in."))
}

async fn new_tab(
    app: &AppHandle,
    registry: &BrowserRegistry,
    caller: &BrowserCaller,
    args: &Value,
    deadline: Deadline,
) -> Result<Value, BrowserError> {
    let asked: NewTabArgs = super::args_of(args)?;
    let workspace = workspace_of(caller)?;
    let id = mint_id()?;
    // The page reports into a channel no JavaScript side holds yet: the strip
    // learns about the tab from the event below, and the pane that opens its
    // chip adopts the page and takes over the reporting.
    let opened = tab::open(
        app,
        registry,
        &id,
        &asked.url,
        &workspace,
        Channel::new(|_| Ok(())),
        deadline,
    )
    .await
    .map_err(|error| host_error(format!("This address could not be opened: {error}")))?;
    let event = TabEvent::Opened {
        browser_id: id.clone(),
        workspace_id: workspace,
        url: opened.url.clone(),
    };
    if let Err(error) = app.emit(TAB_EVENT, &event) {
        eprintln!("devboule: the strip was not told about browser tab {id}: {error}");
    }
    // `browserId` at the top of the result is the whole tab-ownership
    // contract with the daemon: it reads this one key to learn which host owns
    // this tab (see `BrowserOutcome` in the protocol crate).
    Ok(json!({
        "browserId": id,
        "url": opened.url,
        "title": opened.title,
    }))
}

/// The caller's own tabs. Public because the host loop's test drives the real
/// command over a real connection: it is the one command that never touches a
/// page, so it is the one the loop can be driven with without a window.
pub fn list_tabs(
    registry: &BrowserRegistry,
    caller: &BrowserCaller,
) -> Result<Value, BrowserError> {
    let workspace = workspace_of(caller)?;
    let tabs: Vec<Value> = registry
        .tabs_of(&workspace)
        .into_iter()
        .map(|tab| {
            json!({
                "browserId": tab.browser_id,
                "url": tab.url,
                "title": tab.title,
                // Whether the page is the one in front of its pane. The strip's
                // own active pointer lives in the frontend; this is the half
                // Rust can answer without asking it.
                "active": !tab.live.parked(),
            })
        })
        .collect();
    Ok(json!({ "tabs": tabs }))
}

/// Close the tab an agent named, already resolved and held by the caller.
pub fn close_tab(
    app: &AppHandle,
    registry: &BrowserRegistry,
    tab: &TabInfo,
) -> Result<Value, BrowserError> {
    let webview = app
        .get_webview(&tab.label)
        .ok_or_else(|| tab_not_found(&tab.browser_id))?;
    webview
        .close()
        .map_err(|error| host_error(format!("The tab would not close: {error}")))?;
    registry.release(&tab.browser_id);
    super::super::cdp_events::forget(&tab.browser_id);
    scrub::forget(&tab.browser_id);
    if let Err(error) = app.emit(
        TAB_EVENT,
        TabEvent::Closed {
            browser_id: tab.browser_id.clone(),
        },
    ) {
        eprintln!(
            "devboule: the strip was not told browser tab {} closed: {error}",
            tab.browser_id
        );
    }
    Ok(json!({ "closed": true }))
}

/// Take the tab's page somewhere else.
///
/// `Page.navigate` and the history entries are CDP calls rather than the
/// WebView2 back/forward ones, so the answer can report where the page
/// actually landed — and the webview's own gate still sees the navigation,
/// because it is the webview navigating, whoever asked it to.
pub async fn navigate(
    tab: &TabInfo,
    page: &dyn Page,
    args: &Value,
    deadline: Deadline,
) -> Result<Value, BrowserError> {
    let asked: NavigateArgs = super::args_of(args)?;
    // Refused before the page is touched: two ways to go somewhere is a
    // mistake in the call, and answering one of them would hide it.
    if asked.action.is_some() && asked.url.is_some() {
        return Err(host_error(
            "navigate takes a url or an action, not both: send one.",
        ));
    }
    let start = act::read(tab, page).await?;
    act::ready(tab, page).await?;
    match (&asked.action, &asked.url) {
        (Some(action), _) => {
            let action = action.as_str();
            if action == "reload" {
                act::call(page, "Page.reload", json!({})).await?;
            } else {
                history(page, action).await?;
            }
        }
        (None, Some(url)) => {
            let target = super::super::url::accept(url)
                .map_err(|error| refused(BrowserErrorCode::HostError, error))?;
            let landed =
                act::call(page, "Page.navigate", json!({ "url": target.as_str() })).await?;
            if let Some(failed) = landed.get("errorText").and_then(Value::as_str) {
                return Err(host_error(format!(
                    "The page refused to go there: {failed}"
                )));
            }
        }
        (None, None) => return Err(host_error("navigate needs a url or an action.")),
    }
    let answer = act::answer(tab, page, deadline, start, None).await?;
    let delta = answer.get("delta").cloned().unwrap_or(Value::Null);
    Ok(json!({
        "url": act::place(tab).url,
        "title": act::place(tab).title,
        "delta": delta,
    }))
}

/// Step through the page's own history. The entry id comes from the history
/// itself rather than from a counter, so a back from the second entry of a
/// session is the same step as a back from the tenth.
async fn history(page: &dyn Page, action: &str) -> Result<(), BrowserError> {
    let history = act::call(page, "Page.getNavigationHistory", json!({})).await?;
    let current = history
        .get("currentIndex")
        .and_then(Value::as_i64)
        .ok_or_else(|| host_error("This page has no history to step through."))?;
    let step: i64 = match action {
        "back" => -1,
        "forward" => 1,
        other => return Err(host_error(format!("{other} is not a history action."))),
    };
    let nothing_that_way = || host_error("There is nothing that way in this page's history.");
    let target = usize::try_from(current + step).map_err(|_| nothing_that_way())?;
    let entry = history
        .get("entries")
        .and_then(Value::as_array)
        .and_then(|entries| entries.get(target))
        .ok_or_else(nothing_that_way)?;
    let id = entry
        .get("id")
        .cloned()
        .ok_or_else(|| host_error("That history entry has no id."))?;
    act::call(
        page,
        "Page.navigateToHistoryEntry",
        json!({ "entryId": id }),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
#[path = "tabs_tests.rs"]
mod tests;
