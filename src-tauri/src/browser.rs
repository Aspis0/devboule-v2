//! The user-facing browser tab: one child webview per tab inside the main
//! window, parked at a pixel while another tab is in front, and disposed when
//! its tab closes.
//!
//! Everything a page can reach passes `url::gate`, including the navigations
//! this file never sees coming (redirects, `location.assign`, a scripted
//! frame), because the gate is installed as the webview's own
//! `on_navigation` and not only as the address bar's submit check. New-window
//! requests are refused at the webview and reported up the tab's own channel
//! instead of becoming a native popup this app does not manage.

mod ax;
pub mod cdp;
pub mod cdp_events;
mod commands;
mod deadline;
mod delta;
mod delta_input;
mod find;
mod find_query;
mod frames;
pub mod host;
mod live;
mod page_host;
pub(crate) mod registry;
mod tab;
mod tab_guard;
mod tab_reports;
#[cfg(test)]
mod test_pages;
#[cfg(test)]
mod test_support;
mod url;
mod view;
mod view_context;
mod view_line;
mod view_walk;

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State, Wry};

use deadline::Deadline;
use page_host::Act;
use registry::{BrowserRegistry, LogicalRect, PARK_RECT};
use tab::{open, BrowserUpdate, BrowserViewState};

/// The child webview a command is about, resolved through the registry. A
/// disposed tab resolves to nothing, which is the whole of "no navigation
/// after dispose".
fn owned(
    app: &AppHandle,
    registry: &BrowserRegistry,
    id: &str,
) -> Result<tauri::Webview<Wry>, String> {
    let label = registry.label_of(id)?;
    app.get_webview(&label)
        .ok_or_else(|| "This browser tab is no longer open.".to_owned())
}

/// Claim a tab's page for a watcher, and report it.
///
/// A live page is adopted rather than rebuilt: the pane that opens a tab an
/// agent created finds the page already there, so the one-create-per-id rule
/// holds and the user gets the page the agent was reading rather than a second
/// one at the same address.
#[tauri::command]
pub async fn browser_open(
    app: AppHandle,
    registry: State<'_, Arc<BrowserRegistry>>,
    id: String,
    url: String,
    workspace_id: String,
    updates: Channel<BrowserUpdate>,
) -> Result<BrowserViewState, String> {
    if let Some(state) = registry.attach(&id, updates.clone()) {
        return Ok(state);
    }
    open(
        &app,
        &registry,
        &id,
        &url,
        &workspace_id,
        updates,
        Deadline::from_now(),
    )
    .await
}

/// Where a page was put, for the app log. Sizes and positions only: an address
/// is the page's, not this app's to write down.
fn trace_place(id: &str, rect: LogicalRect, state: &str) {
    eprintln!(
        "devboule: debug: browser page {id} at {}x{} +{}+{} ({state})",
        rect.width, rect.height, rect.x, rect.y
    );
}

/// Put the active tab's page over the pane. Every inactive tab stays parked,
/// so exactly one child is visible and the rest keep running at full speed.
#[tauri::command]
pub async fn browser_present(
    app: AppHandle,
    registry: State<'_, Arc<BrowserRegistry>>,
    id: String,
    rect: LogicalRect,
) -> Result<(), String> {
    let _held = tab_guard::hold_for_pane(&registry, &id, tab_guard::PANE_WAIT).await;
    let webview = owned(&app, &registry, &id)?;
    let (position, size) = rect.into_tauri();
    webview.set_position(position).map_err(|e| e.to_string())?;
    webview.set_size(size).map_err(|e| e.to_string())?;
    let overridden = registry.set_rect(&id, rect, false);
    webview.show().map_err(|e| e.to_string())?;
    page_host::raise(&webview);
    if overridden {
        // An agent measured this page while it was parked, and presenting does
        // not take its override off: clear it, then put the page at the pane's
        // size again so the layout is the pane's, whether or not that command
        // is still running.
        let label = registry.label_of(&id)?;
        cdp::clear_override(&cdp::WebviewPage::new(&app, &label)).await;
        webview.set_size(size).map_err(|e| e.to_string())?;
    }
    trace_place(&id, rect, "active");
    Ok(())
}

/// Park a tab's page without stopping it. `hide()` would be one call, but a
/// hidden WebView2 throttles the page's timers to about 1 Hz.
#[tauri::command]
pub async fn browser_park(
    app: AppHandle,
    registry: State<'_, Arc<BrowserRegistry>>,
    id: String,
) -> Result<(), String> {
    let _held = tab_guard::hold_for_pane(&registry, &id, tab_guard::PANE_WAIT).await;
    if registry.rect_of(&id).is_some_and(|(_, parked)| parked) {
        return Ok(());
    }
    let webview = owned(&app, &registry, &id)?;
    let (position, size) = PARK_RECT.into_tauri();
    webview.set_position(position).map_err(|e| e.to_string())?;
    webview.set_size(size).map_err(|e| e.to_string())?;
    registry.set_rect(&id, PARK_RECT, true);
    trace_place(&id, PARK_RECT, "parked");
    Ok(())
}

/// Navigate to a URL the address bar submits. The webview's own gate checks
/// it again, so this one is a friendly refusal, not the boundary.
#[tauri::command]
pub async fn browser_navigate(
    app: AppHandle,
    registry: State<'_, Arc<BrowserRegistry>>,
    id: String,
    url: String,
) -> Result<(), String> {
    let _held = tab_guard::hold_for_pane(&registry, &id, tab_guard::PANE_WAIT).await;
    let webview = owned(&app, &registry, &id)?;
    webview
        .navigate(url::accept(&url)?)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn browser_history(
    app: AppHandle,
    registry: State<'_, Arc<BrowserRegistry>>,
    id: String,
    act: Act,
) -> Result<(), String> {
    let _held = tab_guard::hold_for_pane(&registry, &id, tab_guard::PANE_WAIT).await;
    let label = registry.label_of(&id)?;
    page_host::act(&app, &label, act).await
}

#[tauri::command]
pub async fn browser_reload(
    app: AppHandle,
    registry: State<'_, Arc<BrowserRegistry>>,
    id: String,
) -> Result<(), String> {
    let _held = tab_guard::hold_for_pane(&registry, &id, tab_guard::PANE_WAIT).await;
    owned(&app, &registry, &id)?
        .reload()
        .map_err(|e| e.to_string())
}

/// Close a tab's page. Two orderings, because a create claims its id before
/// the child exists:
///
/// - the child is there: close it natively, and only then drop the entry, so a
///   failed close leaves a claim that owns the label and refuses a reopen over
///   a live webview;
/// - it is not: either this tab was never here, or its create is still
///   building the child. The second is a close the create itself has to
///   honour, so the claim is marked and the create disposes of what it gets.
#[tauri::command]
pub async fn browser_close(
    app: AppHandle,
    registry: State<'_, Arc<BrowserRegistry>>,
    id: String,
) -> Result<(), String> {
    let _held = tab_guard::hold_for_pane(&registry, &id, tab_guard::PANE_WAIT).await;
    let Ok(label) = registry.label_of(&id) else {
        registry.cancel(&id);
        cdp_events::forget(&id);
        return Ok(());
    };
    let Some(webview) = app.get_webview(&label) else {
        registry.cancel(&id);
        cdp_events::forget(&id);
        return Ok(());
    };
    webview.close().map_err(|e| e.to_string())?;
    registry.release(&id);
    cdp_events::forget(&id);
    Ok(())
}
