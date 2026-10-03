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

mod page_host;
pub(crate) mod registry;
mod tab;
mod url;

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State, Wry};

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

/// Open a tab's page. `add_child` blocks until the child exists, so this runs
/// on a worker thread and never on the main one.
#[tauri::command]
pub async fn browser_open(
    app: AppHandle,
    registry: State<'_, BrowserRegistry>,
    id: String,
    url: String,
    updates: Channel<BrowserUpdate>,
) -> Result<BrowserViewState, String> {
    open(&app, &registry, &id, &url, updates).await
}

/// Put the active tab's page over the pane. Every inactive tab stays parked,
/// so exactly one child is visible and the rest keep running at full speed.
#[tauri::command]
pub fn browser_present(
    app: AppHandle,
    registry: State<'_, BrowserRegistry>,
    id: String,
    rect: LogicalRect,
) -> Result<(), String> {
    let webview = owned(&app, &registry, &id)?;
    let (position, size) = rect.into_tauri();
    webview.set_position(position).map_err(|e| e.to_string())?;
    webview.set_size(size).map_err(|e| e.to_string())?;
    registry.set_rect(&id, rect, false);
    webview.show().map_err(|e| e.to_string())?;
    Ok(())
}

/// Park a tab's page without stopping it. `hide()` would be one call, but a
/// hidden WebView2 throttles the page's timers to about 1 Hz.
#[tauri::command]
pub fn browser_park(
    app: AppHandle,
    registry: State<'_, BrowserRegistry>,
    id: String,
) -> Result<(), String> {
    if registry.rect_of(&id).is_some_and(|(_, parked)| parked) {
        return Ok(());
    }
    let webview = owned(&app, &registry, &id)?;
    let (position, size) = PARK_RECT.into_tauri();
    webview.set_position(position).map_err(|e| e.to_string())?;
    webview.set_size(size).map_err(|e| e.to_string())?;
    registry.set_rect(&id, PARK_RECT, true);
    Ok(())
}

/// Navigate to a URL the address bar submits. The webview's own gate checks
/// it again, so this one is a friendly refusal, not the boundary.
#[tauri::command]
pub fn browser_navigate(
    app: AppHandle,
    registry: State<'_, BrowserRegistry>,
    id: String,
    url: String,
) -> Result<(), String> {
    let webview = owned(&app, &registry, &id)?;
    webview
        .navigate(url::accept(&url)?)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn browser_history(
    app: AppHandle,
    registry: State<'_, BrowserRegistry>,
    id: String,
    act: Act,
) -> Result<(), String> {
    let label = registry.label_of(&id)?;
    page_host::act(&app, &label, act).await
}

#[tauri::command]
pub fn browser_reload(
    app: AppHandle,
    registry: State<'_, BrowserRegistry>,
    id: String,
) -> Result<(), String> {
    owned(&app, &registry, &id)?
        .reload()
        .map_err(|e| e.to_string())
}

/// Close a tab's page. Closing twice, or closing after the app already did,
/// reports success: the caller's intent — this tab has no page — holds either
/// way.
#[tauri::command]
pub fn browser_close(
    app: AppHandle,
    registry: State<'_, BrowserRegistry>,
    id: String,
) -> Result<(), String> {
    let Ok(label) = registry.label_of(&id) else {
        return Ok(());
    };
    registry.release(&id);
    if let Some(webview) = app.get_webview(&label) {
        webview.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}
